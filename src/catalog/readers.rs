//! The TVIEWs reading each table, with the triggers each needs there: what the
//! one set of `pg_tviews` triggers on a table serves.

use super::PlanEntry;
use crate::lineage::{KeyMapping, MappingKind};
use pgrx::pg_sys::Oid;

/// Which triggers a table gets for a TVIEW reading it, from how its writes map
/// to keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerSet {
    /// The key is read off each changed row: row trigger.
    Row,
    /// A query maps the statement's changed rows: delta triggers. A partitioned
    /// table maps row by row: transition tables of its statement triggers would
    /// see only the rows of statements naming it, and its partitions get no copy
    /// of a trigger with transition tables.
    Delta,
    /// Another TVIEW's table: its refreshes are mapped by the delta
    /// triggers alone. They fire inside the flush, which drains what they queue;
    /// a flush or TRUNCATE trigger there would flush again from inside it.
    TviewDelta,
    /// Refreshing another TVIEW refreshes this one: nothing.
    None,
}

impl TriggerSet {
    /// The set a table whose writes map as `kind` gets; `tview` when it is
    /// another TVIEW's table.
    #[must_use]
    pub const fn of(kind: MappingKind, tview: bool) -> Self {
        match (kind, tview) {
            (MappingKind::Mapped | MappingKind::AllKeys, true) => Self::TviewDelta,
            (MappingKind::Mapped | MappingKind::AllKeys, false) => Self::Delta,
            (MappingKind::Local, false) => Self::Row,
            (MappingKind::Local | MappingKind::Propagated, _) => Self::None,
        }
    }

    /// The set of the table `mapping` maps.
    #[must_use]
    pub const fn of_mapping(mapping: &KeyMapping) -> Self {
        Self::of(mapping.kind, mapping.tview.is_some())
    }
}

/// A TVIEW reading a table, and the set it needs there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub entity: String,
    pub set: TriggerSet,
}

/// The TVIEWs reading one table (the root of a partition tree), in entity
/// order, each with the triggers it needs there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableReaders {
    /// The table is partitioned: a `Delta` reader maps row by row.
    pub partitioned: bool,
    pub readers: Vec<Reader>,
}

impl TableReaders {
    /// The readers of `relid` among `plans`: each TVIEW whose plan maps the
    /// table to a set of triggers.
    #[must_use]
    pub fn from_plans(plans: &[PlanEntry], relid: Oid, partitioned: bool) -> Self {
        let mut readers: Vec<Reader> = plans
            .iter()
            .filter_map(|entry| {
                let mapping = entry
                    .plan
                    .tables
                    .iter()
                    .find(|t| t.relid == relid.to_u32())?;
                let set = TriggerSet::of_mapping(mapping);
                (set != TriggerSet::None).then(|| Reader {
                    entity: entry.entity.clone(),
                    set,
                })
            })
            .collect();
        readers.sort_by(|a, b| a.entity.cmp(&b.entity));
        Self {
            partitioned,
            readers,
        }
    }

    fn serving(&self, serves: impl Fn(TriggerSet) -> bool) -> impl Iterator<Item = &str> {
        self.readers
            .iter()
            .filter(move |r| serves(r.set))
            .map(|r| r.entity.as_str())
    }

    /// The readers the row trigger serves.
    pub fn row(&self) -> impl Iterator<Item = &str> {
        let partitioned = self.partitioned;
        self.serving(move |set| set == TriggerSet::Row || (partitioned && set == TriggerSet::Delta))
    }

    /// The readers the delta triggers serve.
    pub fn delta(&self) -> impl Iterator<Item = &str> {
        let partitioned = self.partitioned;
        self.serving(move |set| {
            set == TriggerSet::TviewDelta || (!partitioned && set == TriggerSet::Delta)
        })
    }

    /// The readers the `TRUNCATE` trigger serves.
    pub fn truncate(&self) -> impl Iterator<Item = &str> {
        self.serving(|set| matches!(set, TriggerSet::Row | TriggerSet::Delta))
    }
}

/// Every TVIEW's plan as the catalog holds it now, not the backend's cache:
/// what the triggers of a table are made from. A plan that does not decode is
/// left out, its entity returned beside (the health check names it).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn plans_now() -> crate::TViewResult<(Vec<PlanEntry>, Vec<String>)> {
    let rows = crate::utils::spi::catalog_rows(
        &format!(
            "SELECT entity, plan FROM {} ORDER BY entity",
            super::meta_table()
        ),
        &[],
        |row| {
            let entity = row.named::<String>("entity")?.unwrap_or_default();
            let plan = row
                .named::<pgrx::JsonB>("plan")?
                .map_or(serde_json::Value::Null, |j| j.0);
            Ok(match crate::lineage::plan::TviewPlan::decode(&entity, plan) {
                Ok(plan) => Ok(PlanEntry { entity, plan }),
                Err(_) => Err(entity),
            })
        },
    )?;
    let mut plans = Vec::with_capacity(rows.len());
    let mut unreadable = Vec::new();
    for row in rows {
        match row {
            Ok(entry) => plans.push(entry),
            Err(entity) => unreadable.push(entity),
        }
    }
    Ok((plans, unreadable))
}

/// The tables `entity`'s stored plan maps, read by the SQL plan decoder: what
/// its triggers were made for, even when the plan does not decode in full.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn tables_of(entity: &str) -> crate::TViewResult<Vec<Oid>> {
    crate::utils::spi::oids(
        &format!(
            "SELECT DISTINCT e.relid::pg_catalog.oid FROM {} m, \
                    {}.pg_tviews_plan_tables(m.plan) e \
             WHERE m.entity = $1 AND e.relid IS NOT NULL",
            super::meta_table(),
            crate::utils::ext_schema(),
        ),
        &[crate::utils::spi::text(entity)],
    )
}

#[cfg(test)]
mod tests {
    use super::{Reader, TableReaders, TriggerSet};
    use crate::catalog::PlanEntry;
    use crate::lineage::{KeyMapping, MappingKind, plan::TviewPlan};
    use pgrx::pg_sys::Oid;

    fn entry(entity: &str, tables: &[(u32, MappingKind, bool)]) -> PlanEntry {
        PlanEntry {
            entity: entity.to_string(),
            plan: TviewPlan {
                tables: tables
                    .iter()
                    .map(|&(relid, kind, tview)| KeyMapping {
                        relid,
                        kind,
                        tview: tview.then(|| "other".to_string()),
                        ..KeyMapping::default()
                    })
                    .collect(),
                ..TviewPlan::default()
            },
        }
    }

    fn reader(entity: &str, set: TriggerSet) -> Reader {
        Reader {
            entity: entity.to_string(),
            set,
        }
    }

    #[test]
    fn every_tview_reading_a_table_is_a_reader_with_its_set_in_entity_order() {
        let plans = [
            entry("post", &[(10, MappingKind::Local, false)]),
            entry("feed", &[(10, MappingKind::Mapped, false)]),
            entry("author", &[(20, MappingKind::Mapped, true)]),
            entry("tag", &[(10, MappingKind::Propagated, false)]),
            entry("note", &[(30, MappingKind::Local, false)]),
        ];
        assert_eq!(
            TableReaders::from_plans(&plans, Oid::from(10), false).readers,
            vec![
                reader("feed", TriggerSet::Delta),
                reader("post", TriggerSet::Row),
            ]
        );
        assert_eq!(
            TableReaders::from_plans(&plans, Oid::from(20), false).readers,
            vec![reader("author", TriggerSet::TviewDelta)]
        );
        assert!(
            TableReaders::from_plans(&plans, Oid::from(40), false)
                .readers
                .is_empty()
        );
    }

    #[test]
    fn each_trigger_serves_the_readers_of_its_kind() {
        let plans = [
            entry("a", &[(10, MappingKind::Local, false)]),
            entry("b", &[(10, MappingKind::AllKeys, false)]),
            entry("c", &[(10, MappingKind::Mapped, true)]),
        ];
        let plain = TableReaders::from_plans(&plans, Oid::from(10), false);
        assert_eq!(plain.row().collect::<Vec<_>>(), ["a"]);
        assert_eq!(plain.delta().collect::<Vec<_>>(), ["b", "c"]);
        assert_eq!(plain.truncate().collect::<Vec<_>>(), ["a", "b"]);
        // A partitioned table maps its `Delta` readers row by row.
        let partitioned = TableReaders::from_plans(&plans, Oid::from(10), true);
        assert_eq!(partitioned.row().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(partitioned.delta().collect::<Vec<_>>(), ["c"]);
        assert_eq!(partitioned.truncate().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn the_set_of_a_mapping_kind() {
        assert_eq!(TriggerSet::of(MappingKind::Local, false), TriggerSet::Row);
        assert_eq!(
            TriggerSet::of(MappingKind::Mapped, false),
            TriggerSet::Delta
        );
        assert_eq!(
            TriggerSet::of(MappingKind::AllKeys, false),
            TriggerSet::Delta
        );
        assert_eq!(
            TriggerSet::of(MappingKind::Mapped, true),
            TriggerSet::TviewDelta
        );
        assert_eq!(
            TriggerSet::of(MappingKind::AllKeys, true),
            TriggerSet::TviewDelta
        );
        assert_eq!(TriggerSet::of(MappingKind::Local, true), TriggerSet::None);
        assert_eq!(
            TriggerSet::of(MappingKind::Propagated, false),
            TriggerSet::None
        );
    }
}

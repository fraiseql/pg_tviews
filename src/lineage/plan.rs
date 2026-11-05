//! A TVIEW's propagation plan (ADR 0203): everything registration derives from
//! its backing view's query tree, one typed value assembled from the lineage
//! ([`assemble`]), stored as one versioned document in `pg_tview_meta.plan` and
//! read back by the triggers and the flush.

use super::{IdentityKind, Lineage, TableKind};
use pgrx::pg_sys::Oid;
use serde::{Deserialize, Serialize};

/// The version this library writes. It also reads version 1, a plan without
/// references, and version 2, one without the columns its rows are linked by
/// (what the upgrade scripts of earlier releases write before they re-derive);
/// a plan of any other version is re-derived by `ALTER EXTENSION pg_tviews
/// UPDATE`, never read.
pub const PLAN_VERSION: u32 = 3;

/// What a TVIEW's definition says about propagating writes to its rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TviewPlan {
    pub version: u32,
    /// The rows come from the branches of a set operation (UNION, INTERSECT,
    /// EXCEPT): they are recomputed, never patched.
    #[serde(default)]
    pub set_operation: bool,
    /// The TVIEWs it embeds.
    #[serde(default)]
    pub embeds: Vec<PlanEmbed>,
    /// The direct-patch map: `(column of the table holding the identity, data
    /// key)` for the columns copied into `data` and read nowhere else.
    #[serde(default)]
    pub direct: Vec<(String, String)>,
    /// How a write to each base table maps to its keys (ADR 0157): what the
    /// statement trigger runs.
    #[serde(default)]
    pub tables: Vec<crate::lineage::KeyMapping>,
    /// The tables whose rows carry the key: what the row trigger reads.
    #[serde(default)]
    pub paths: Vec<LocalPath>,
    /// The output columns a foreign key of the root table proves to hold another
    /// table's key (`tviews.registry.references` publishes those naming a TVIEW).
    #[serde(default)]
    pub references: Vec<crate::lineage::Reference>,
    /// The TVIEW's row key, `pk_<entity>`, and its type.
    #[serde(default)]
    pub key: PlanKey,
    /// The `default_text_search_config` it was created under, which its
    /// refreshes run under (`to_tsvector(text)` reads it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_search: Option<String>,
}

/// A TVIEW's row key: the integer column `pk_<entity>` (ADR 0169; smallint,
/// integer, bigint or a domain over one), its rows' primary key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanKey {
    pub column: String,
    /// The column's type as `format_type` writes it; empty in a plan stored
    /// before the key was recorded.
    #[serde(rename = "type", default)]
    pub type_name: String,
}

/// The row key column of `entity`'s TVIEW, `pk_<entity>`: the one place the
/// convention turns an entity name into a column name. What reads a TVIEW's
/// registered plan reads [`TviewPlan::key`] instead.
#[must_use]
pub fn key_column(entity: &str) -> String {
    format!("pk_{entity}")
}

/// Another TVIEW this one embeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEmbed {
    pub entity: String,
    /// The output columns of this TVIEW equal to the child's `pk_<child>`, one
    /// per read of the child: the parents of a refreshed child row are the rows
    /// holding its key in one of them.
    pub lookups: Vec<String>,
    pub kind: crate::lineage::EmbedKind,
    /// Where the child's document lands in `data` (empty for a scalar embed).
    #[serde(default)]
    pub path: Vec<String>,
}

/// A table whose changed row holds a key of the TVIEW in `initial_col`: the
/// table holding the identity (`root`), a table joined on the key, or a source of
/// an aggregate TVIEW's group key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPath {
    pub source_oid: Oid,
    pub source_table: String,
    pub entity_name: String,
    pub initial_col: String,
    /// The columns of the table the TVIEW reads: an UPDATE touching none of
    /// them changes no row. Empty means unknown: every UPDATE refreshes.
    #[serde(default)]
    pub source_columns: Vec<String>,
    /// The table holds the TVIEW's identity: its own rows are the TVIEW's rows.
    #[serde(default)]
    pub root: bool,
    /// The attribute number of `initial_col` when registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_attnum: Option<i16>,
}

/// How an UPDATE of a mapped table is written into every TVIEW row it reaches
/// in one statement: the rows whose `lookup_col` holds the changed
/// row's key, each changed column in `fields` written at its path in `data`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanoutPatch {
    pub lookup_col: String,
    /// `(source column, path in data)` pairs copied unchanged; a column copied
    /// to several paths has a pair for each.
    pub fields: Vec<(String, Vec<String>)>,
    /// For a table two equalities away, through a table in between: the query
    /// template from the changed rows' key values (`$1`, `int8[]`) to the
    /// `lookup_col` values they reach, as `(key, lookup value)` pairs (#228).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

/// How writes to a table map to a registered TVIEW's keys: the stored form of a
/// [`TableKind`]. Any other name fails to decode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingKind {
    /// The key is a column of the changed row (row trigger).
    #[default]
    Local,
    /// A mapping query over the transition table (`sql`).
    Mapped,
    /// Reached only through an embed: no trigger.
    Propagated,
    /// No cascade maps the table: the policy applies.
    AllKeys,
}

/// One table of a registered TVIEW's `key_mappings`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeyMapping {
    pub relid: u32,
    /// The table's qualified name: what a restore rebinds `relid` from.
    pub table: String,
    pub kind: MappingKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    /// `all_keys`: why no cascade maps the table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `mapped` (and `all_keys` for its traceable reads): the query template (see
    /// [`super::render_template`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    /// Columns of the table the TVIEW reads; empty when unknown.
    #[serde(default)]
    pub attnums: Vec<i16>,
    #[serde(default)]
    pub columns: Vec<String>,
    /// `mapped` through one equality onto a column of the root: `(this table's
    /// column, the root's column)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop: Option<(String, String)>,
    /// The column of this table whose value the fan-out patch looks up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_col: Option<String>,
    /// How an UPDATE is written into every TVIEW row it reaches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanout: Option<FanoutPatch>,
    /// The table of another TVIEW, of that entity: refreshed first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tview: Option<String>,
    /// What a refresh reads of the table, for value locks (ADR 0207).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<super::ReadSet>,
    /// The columns of the table the TVIEW's rows are linked by (joins, filters,
    /// groupings…): a write that changes none of them, and inserts or deletes no
    /// row, changes no link (#263). `None` when unknown: every write does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<i16>>,
    /// `all_keys` through reads that only read rows meeting conditions against
    /// constants: the query template over the changed rows returning a row when
    /// one meets them. The uncascaded policy applies only then (#284).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
}

/// The row key of `entity`'s backing view `view_oid`: `pk_<entity>` with the
/// type the view gives it.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn view_key(entity: &str, view_oid: Oid) -> crate::TViewResult<PlanKey> {
    let column = key_column(entity);
    let type_name = crate::utils::spi::catalog_one::<String>(
        "SELECT pg_catalog.format_type(a.atttypid, a.atttypmod) FROM pg_catalog.pg_attribute a \
         WHERE a.attrelid = $1 AND a.attname = $2 AND NOT a.attisdropped",
        &[
            crate::utils::spi::oid(view_oid),
            crate::utils::spi::text(column.as_str()),
        ],
    )?
    .unwrap_or_default();
    Ok(PlanKey { column, type_name })
}

/// The plan of `entity` from its lineage: its key mappings with their fan-out
/// patches, its embeds, its direct-patch map and its references, with `paths`,
/// the tables whose rows carry its key, and `key`, its row key.
#[must_use]
pub fn assemble(entity: &str, lineage: &Lineage, paths: Vec<LocalPath>, key: PlanKey) -> TviewPlan {
    TviewPlan {
        version: PLAN_VERSION,
        set_operation: lineage.set_operation,
        embeds: lineage
            .embeds(entity)
            .into_iter()
            .map(|e| PlanEmbed {
                entity: e.entity,
                lookups: e.lookups,
                kind: e.kind,
                path: e.path,
            })
            .collect(),
        direct: lineage.direct_fields(),
        tables: key_mappings(lineage),
        paths,
        references: lineage.references.clone(),
        key,
        text_search: None,
    }
}

impl MappingKind {
    /// The stored kind of a table's [`TableKind`].
    #[must_use]
    pub const fn of(kind: &TableKind) -> Self {
        match kind {
            TableKind::Local(_) => Self::Local,
            TableKind::Mapped => Self::Mapped,
            TableKind::Propagated(_) => Self::Propagated,
            TableKind::AllKeys(_) => Self::AllKeys,
        }
    }
}

/// One key mapping per base table of `lineage`.
fn key_mappings(lineage: &Lineage) -> Vec<KeyMapping> {
    lineage
        .tables
        .iter()
        .map(|t| {
            let (column, entity, reason, sql, hop) = match &t.kind {
                TableKind::Local(column) => (Some(column.clone()), None, None, None, None),
                TableKind::Propagated(entity) => (None, Some(entity.clone()), None, None, None),
                TableKind::AllKeys(reason) => {
                    (None, None, Some(reason.clone()), t.sql.clone(), None)
                }
                TableKind::Mapped => (
                    None,
                    None,
                    None,
                    Some(t.sql.clone().unwrap_or_default()),
                    t.hop.clone(),
                ),
            };
            let (key_col, fanout) = match fanout_patch(lineage, t) {
                Some((own, patch)) => (Some(own), Some(patch)),
                None => (None, None),
            };
            KeyMapping {
                relid: t.relid,
                table: t.qualified.clone(),
                kind: MappingKind::of(&t.kind),
                column,
                entity,
                reason,
                sql,
                attnums: t.columns.iter().map(|(_, n)| *n).collect(),
                columns: t.columns.iter().map(|(name, _)| name.clone()).collect(),
                hop,
                key_col,
                fanout,
                tview: t.tview.clone(),
                reads: t.reads.clone(),
                links: t.links.clone(),
                guard: match t.kind {
                    TableKind::AllKeys(_) => t.guard.clone(),
                    _ => None,
                },
            }
        })
        .collect()
}

/// The fan-out patch of `table`, with the column of it whose value the patch
/// looks up. A `mapped` table one equality away from a column of the table
/// holding the identity, which the TVIEW projects, or two through a table in
/// between, gets one: an UPDATE of its columns copied into `data` is written into
/// every TVIEW row with that column's value. Not for a DISTINCT ON TVIEW, whose
/// row shows its group's winner, a set operation, nor for the virtual generated
/// columns a trigger sees as NULL.
fn fanout_patch(lineage: &Lineage, table: &super::TableLineage) -> Option<(String, FanoutPatch)> {
    if lineage.identity.kind == IdentityKind::DistinctOn || lineage.set_operation {
        return None;
    }
    let (Some((own, _)), Some((lookup_col, fields))) = (&table.hop, &table.fanout) else {
        return None;
    };
    let fields: Vec<(String, Vec<String>)> = fields
        .iter()
        .filter(|(col, _)| !table.virtual_reads.contains(col))
        .cloned()
        .collect();
    if fields.is_empty() {
        return None;
    }
    Some((
        own.clone(),
        FanoutPatch {
            lookup_col: lookup_col.clone(),
            fields,
            via: table.via.clone(),
        },
    ))
}

/// What a plan names that a restored database lacks, or numbers without naming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unbound {
    /// A table, by its qualified name.
    Table(String),
    /// A column of a table, by name.
    Column(String, String),
    /// A column number of a table that the plan stores no name for.
    Unnamed(String, i16),
    /// A path from a table the plan maps no write of.
    Path(String),
}

impl TviewPlan {
    /// Every column the plan numbers, `(relid, attnum, name)`: each table's
    /// `attnums` beside its `columns`, each path's `initial_attnum` beside its
    /// `initial_col`.
    #[must_use]
    pub fn numbered_columns(&self) -> Vec<(u32, i16, String)> {
        let mut out: Vec<(u32, i16, String)> = self
            .tables
            .iter()
            .flat_map(|t| {
                t.attnums
                    .iter()
                    .zip(&t.columns)
                    .map(|(&attnum, name)| (t.relid, attnum, name.clone()))
            })
            .chain(self.paths.iter().filter_map(|p| {
                p.initial_attnum
                    .map(|attnum| (p.source_oid.to_u32(), attnum, p.initial_col.clone()))
            }))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The plan with its relations and column numbers rebound: `relid` gives a
    /// table's new relid, `attnum` a column's new number from its old relid and
    /// number. Templates (`{r:…}`, `{c:…:…}`, guards included), `attnums`,
    /// `links`, the read sets' `attnum` (0, the table itself, stays) and the paths
    /// are rewritten.
    ///
    /// # Errors
    /// What has no new value: a column number the plan does not name, a path
    /// from a table it maps no write of.
    pub fn rebound(
        &self,
        relid: &dyn Fn(u32) -> Option<u32>,
        attnum: &dyn Fn(u32, i16) -> Option<i16>,
    ) -> Result<Self, Unbound> {
        let mut plan = self.clone();
        for table in &mut plan.tables {
            let old = table.relid;
            let name = table.table.clone();
            let number = |a: i16| attnum(old, a).ok_or_else(|| Unbound::Unnamed(name.clone(), a));
            let template = |sql: &str| {
                super::rewrite_template(sql, &|p| match p {
                    super::Placeholder::Relation(r) => relid(r).map(super::Placeholder::Relation),
                    super::Placeholder::Column(r, a) => {
                        Some(super::Placeholder::Column(relid(r)?, attnum(r, a)?))
                    }
                })
                .ok_or_else(|| Unbound::Unnamed(name.clone(), 0))
            };
            table.relid = relid(old).ok_or_else(|| Unbound::Table(name.clone()))?;
            table.attnums = table
                .attnums
                .iter()
                .map(|&a| number(a))
                .collect::<Result<_, _>>()?;
            if let Some(links) = &table.links {
                table.links = Some(links.iter().map(|&a| number(a)).collect::<Result<_, _>>()?);
            }
            for read in &mut table.reads {
                if read.attnum != 0 {
                    read.attnum = number(read.attnum)?;
                }
                if let Some(sql) = &read.sql {
                    read.sql = Some(template(sql)?);
                }
            }
            if let Some(sql) = &table.sql {
                table.sql = Some(template(sql)?);
            }
            if let Some(guard) = &table.guard {
                table.guard = Some(template(guard)?);
            }
            if let Some(fanout) = &mut table.fanout
                && let Some(via) = &fanout.via
            {
                fanout.via = Some(template(via)?);
            }
        }
        for path in &mut plan.paths {
            let old = path.source_oid.to_u32();
            if !self.tables.iter().any(|t| t.relid == old) {
                return Err(Unbound::Path(path.source_table.clone()));
            }
            path.source_oid =
                Oid::from(relid(old).ok_or_else(|| Unbound::Table(path.source_table.clone()))?);
            if let Some(a) = path.initial_attnum {
                path.initial_attnum = Some(
                    attnum(old, a).ok_or_else(|| Unbound::Unnamed(path.source_table.clone(), a))?,
                );
            }
        }
        Ok(plan)
    }
}

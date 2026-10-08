//! What registration derives from a TVIEW's lineage: its propagation plan (ADR
//! 0203) and the tables no cascade reaches.

use crate::catalog::plan::{LocalPath, PLAN_VERSION, PlanEmbed, TviewPlan};
use crate::ddl::uncascaded::Declarations;
use crate::error::TViewError;
use crate::error::TViewResult;
use pgrx::pg_sys;

/// What registration derives from a definition: its lineage, and the plan stored
/// from it.
pub(crate) struct Derivation {
    pub(crate) lineage: crate::lineage::Lineage,
    pub(crate) plan: TviewPlan,
    /// The base tables the view reads (`pg_depend`), and those the functions it
    /// calls read (#193): the tables its triggers go on.
    pub(crate) base_tables: Vec<pg_sys::Oid>,
    /// The functions it calls that may read tables and are not declared (#193).
    pub(crate) undeclared_functions: Vec<String>,
}

/// The aggregate TVIEWs (issue #58) a definition embeds, each mapped to the output
/// column that carries the value joined to the aggregate's `pk_<aggregate>`
/// (issue #126). An aggregate has no `fk_<aggregate>` column to propagate by, so a
/// change to group `k` refreshes the rows whose column equals `k`.
///
/// # Errors
/// Rejects a definition that reads an aggregate TVIEW without projecting the
/// column equal to its key: such a TVIEW could never be refreshed when the
/// aggregate changes.
pub(crate) fn aggregate_embeds(
    lineage: &crate::lineage::Lineage,
    entity_name: &str,
) -> TViewResult<std::collections::BTreeMap<String, String>> {
    lineage
        .aggregate_embeds
        .iter()
        .map(|(aggregate, column)| match column {
            Some(column) => Ok((aggregate.clone(), column.clone())),
            None => Err(TViewError::DefinitionRefused {
                reason: format!(
                    "TVIEW 'tv_{entity_name}' reads aggregate TVIEW '{aggregate}' but no output \
                     column carries the value it is joined to on pk_{aggregate}, so a change to \
                     a '{aggregate}' group could not be routed to the rows embedding it. Join \
                     tv_{aggregate} with an equality on its key (e.g. `LEFT JOIN \
                     tv_{aggregate} a ON a.pk_{aggregate} = t.pk_{entity_name}`) and project \
                     the other side of that equality."
                ),
            }),
        })
        .collect()
}

/// Analyze the backing view `view_oid` (ADR 0157) and derive its plan: the local
/// paths of its local tables (for an aggregate TVIEW, one per declared group key,
/// issue #58), the mapping of every base table, its embeds and its direct-patch
/// map.
pub(crate) fn derive(
    entity_name: &str,
    group_keys: Option<&crate::ddl::aggregate::GroupKeys>,
    base_tables: &[pg_sys::Oid],
    view_oid: pg_sys::Oid,
    declarations: &Declarations,
) -> TViewResult<Derivation> {
    let mut lineage = crate::lineage::analyze(entity_name, view_oid, base_tables)?;
    aggregate_embeds(&lineage, entity_name)?;
    let (function_tables, undeclared_functions) =
        crate::ddl::uncascaded::apply_function_reads(entity_name, declarations, &mut lineage)?;
    let mut all_base_tables = base_tables.to_vec();
    for table in function_tables {
        if !all_base_tables.contains(&table) {
            all_base_tables.push(table);
        }
    }
    let paths = match group_keys {
        Some(keys) => crate::ddl::aggregate::local_paths(entity_name, keys, base_tables, view_oid)?,
        None => local_paths(entity_name, &lineage),
    };
    let mut key_mappings = lineage.to_json();
    add_fanout_patches(&mut key_mappings, &lineage);
    let tables = serde_json::from_value(key_mappings).map_err(|e| TViewError::CatalogError {
        operation: format!("Derive the key mappings of tv_{entity_name}"),
        pg_error: e.to_string(),
    })?;
    let plan = TviewPlan {
        version: PLAN_VERSION,
        set_operation: lineage.set_operation,
        embeds: lineage
            .embeds(entity_name)
            .into_iter()
            .map(|e| PlanEmbed {
                entity: e.entity,
                lookups: e.lookups,
                kind: e.kind,
                path: e.path,
            })
            .collect(),
        direct: lineage.direct_fields(),
        tables,
        paths,
    };
    Ok(Derivation {
        lineage,
        plan,
        base_tables: all_base_tables,
        undeclared_functions,
    })
}

/// A `mapped` table one equality away from a column of the table holding the
/// identity, which the TVIEW projects, gets the fan-out patch of issue #120: an
/// UPDATE of its columns copied into `data` is written into every TVIEW row with
/// that column's value. Not for a DISTINCT ON TVIEW, whose row shows its group's
/// winner, nor for the virtual generated columns a trigger sees as NULL (#179).
pub(crate) fn add_fanout_patches(
    key_mappings: &mut serde_json::Value,
    lineage: &crate::lineage::Lineage,
) {
    if lineage.identity.kind == crate::lineage::IdentityKind::DistinctOn || lineage.set_operation {
        return;
    }
    let Some(entries) = key_mappings.as_array_mut() else {
        return;
    };
    for table in &lineage.tables {
        let (Some((own, _)), Some((lookup_col, fields))) = (&table.hop, &table.fanout) else {
            continue;
        };
        let fields: Vec<(String, String)> = fields
            .iter()
            .filter(|(col, _)| !table.virtual_reads.contains(col))
            .cloned()
            .collect();
        if fields.is_empty() {
            continue;
        }
        let fanout = crate::catalog::plan::FanoutPatch {
            lookup_col: lookup_col.clone(),
            fields,
        };
        if let Some(entry) = entries
            .iter_mut()
            .find(|e| e["relid"].as_u64() == Some(u64::from(table.relid)))
        {
            entry["key_col"] = own.clone().into();
            entry["fanout"] = serde_json::to_value(&fanout).unwrap_or_default();
        }
    }
}

/// One local path per local table: the key is the table's `column`, read off the
/// changed row; an UPDATE that touches none of the columns the TVIEW reads is
/// skipped. The table holding the key (of each UNION branch) gets one too, marked
/// `root`: its own rows are the TVIEW's rows (ADR 0169).
pub(crate) fn local_paths(entity_name: &str, lineage: &crate::lineage::Lineage) -> Vec<LocalPath> {
    lineage
        .tables
        .iter()
        .filter_map(|t| match &t.kind {
            crate::lineage::TableKind::Local(column) => Some(LocalPath {
                source_oid: pg_sys::Oid::from(t.relid),
                source_table: t.relname.clone(),
                entity_name: entity_name.to_string(),
                initial_col: column.clone(),
                source_columns: t.columns.iter().map(|(name, _)| name.clone()).collect(),
                root: t.root,
                initial_attnum: t.columns.iter().find(|(n, _)| n == column).map(|(_, a)| *a),
            }),
            _ => None,
        })
        .collect()
}

/// The tables of `lineage` no cascade reaches, as the policy reports them.
pub(crate) fn uncascaded_tables(
    lineage: &crate::lineage::Lineage,
) -> Vec<crate::ddl::uncascaded::UncascadedTable> {
    lineage
        .all_keys()
        .into_iter()
        .map(
            |(relid, name, reason)| crate::ddl::uncascaded::UncascadedTable {
                oid: pg_sys::Oid::from(relid),
                name,
                reason,
            },
        )
        .collect()
}

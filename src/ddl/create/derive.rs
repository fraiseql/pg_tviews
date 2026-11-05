//! What registration derives from a TVIEW's lineage: its propagation plan (ADR
//! 0203) and the tables no cascade reaches.

use crate::ddl::uncascaded::Declarations;
use crate::error::TViewError;
use crate::error::TViewResult;
use crate::lineage::plan::{LocalPath, TviewPlan};
use pgrx::pg_sys;

/// What registration derives from a definition: its lineage, and the plan stored
/// from it.
pub(crate) struct Derivation {
    pub(crate) lineage: crate::lineage::Lineage,
    pub(crate) plan: TviewPlan,
    /// The functions it calls that may read tables and are not declared.
    pub(crate) undeclared_functions: Vec<String>,
}

/// The aggregate TVIEWs a definition embeds, each mapped to the output
/// column that carries the value joined to the aggregate's `pk_<aggregate>`.
/// An aggregate has no `fk_<aggregate>` column to propagate by, so a
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

/// [`derive`] in an owner frame of `owner`, the TVIEW's owner (or owner-to-be).
///
/// # Errors
/// What [`derive`] returns.
pub(crate) fn derive_as(
    owner: crate::owner::Owner,
    entity_name: &str,
    group_keys: Option<&crate::ddl::aggregate::GroupKeys>,
    base_tables: &[pg_sys::Oid],
    view_oid: pg_sys::Oid,
    declarations: &Declarations,
) -> TViewResult<Derivation> {
    crate::owner::AsOwner::run(owner, |frame| {
        derive(
            frame,
            entity_name,
            group_keys,
            base_tables,
            view_oid,
            declarations,
        )
    })
}

/// Analyze the backing view `view_oid` (ADR 0157) and derive its plan: the local
/// paths of its local tables (for an aggregate TVIEW, one per declared group
/// key), the mapping of every base table, its embeds and its direct-patch
/// map.
fn derive(
    frame: &crate::owner::OwnerFrame,
    entity_name: &str,
    group_keys: Option<&crate::ddl::aggregate::GroupKeys>,
    base_tables: &[pg_sys::Oid],
    view_oid: pg_sys::Oid,
    declarations: &Declarations,
) -> TViewResult<Derivation> {
    let others = crate::catalog::registered::all()?;
    let mut lineage = crate::lineage::analyze(frame, entity_name, view_oid, base_tables, &others)?;
    aggregate_embeds(&lineage, entity_name)?;
    let undeclared_functions =
        crate::ddl::uncascaded::apply_function_reads(entity_name, declarations, &mut lineage)?;
    let mut paths = local_paths(entity_name, &lineage);
    if let Some(keys) = group_keys {
        crate::ddl::aggregate::check_group_keys(entity_name, keys, &paths)?;
        // The rows of an aggregate are its groups, not any table's rows.
        for path in &mut paths {
            path.root = false;
        }
    }
    let key = crate::lineage::plan::view_key(entity_name, view_oid)?;
    let plan = crate::lineage::plan::assemble(entity_name, &lineage, paths, key);
    Ok(Derivation {
        lineage,
        plan,
        undeclared_functions,
    })
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
            |(relid, name, reason, followed)| crate::ddl::uncascaded::UncascadedTable {
                oid: pg_sys::Oid::from(relid),
                name,
                reason,
                followed,
            },
        )
        .collect()
}

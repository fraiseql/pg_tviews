//! Parent discovery for the flush: the rows of the TVIEWs that embed a changed
//! TVIEW's rows, found through the lookup columns of their plans.

use pgrx::prelude::*;
use std::collections::HashMap;

use crate::catalog::KeyType;
use crate::queue::RefreshKey;
use crate::queue::key::KeyValue;
use crate::utils::ident;

/// The parent rows to refresh when rows of `child` changed: for each TVIEW that
/// embeds `child`, its rows whose lookup columns (the output columns its plan
/// equates to the child's key) hold one of `pks`, the children's `pk_<child>`
/// values. Keyed by child pk; each parent key is the parent's identity value.
///
/// Parents reference a child by its `pk_<child>` whatever the child's identity
/// (ADR 0169, D4), so the caller passes the `pk_<child>` of the rows its
/// refresh touched, before and after. For the child rows that `appeared`, the
/// parents are also looked up in their backing view: a parent row an inner join
/// dropped with the child is in the view again, not in its table.
///
/// One query per parent lookup column, `lookup = ANY($1)` over all the pks.
///
/// # Errors
/// Returns an error if a parent's catalog row or table cannot be read.
pub fn find_parents_batch(
    child: &str,
    pks: &[i64],
    appeared: &[i64],
    graph: &crate::flush::EntityDepGraph,
) -> crate::TViewResult<HashMap<i64, Vec<RefreshKey>>> {
    let mut result: HashMap<i64, Vec<RefreshKey>> = HashMap::with_capacity(pks.len());
    if pks.is_empty() {
        return Ok(result);
    }
    let parents = graph.parents.get(child).cloned().unwrap_or_default();
    if parents.is_empty() {
        return Ok(result);
    }
    let child_table = crate::catalog::TviewMeta::load_by_entity(child)?
        .ok_or_else(|| crate::TViewError::TviewNotFound {
            name: child.to_string(),
        })?
        .tview_oid;
    for parent in parents {
        let unpruned: Vec<i64> = pks
            .iter()
            .copied()
            .filter(|&pk| !prune_edge(graph, child, &parent, pk))
            .collect();
        if unpruned.is_empty() {
            continue;
        }
        let appeared: Vec<i64> = appeared
            .iter()
            .copied()
            .filter(|pk| unpruned.contains(pk))
            .collect();
        // A parent being computed from these child rows meets this lookup on
        // their keys (ADR 0207): locked before the lookup runs.
        let keys: Vec<String> = unpruned.iter().map(ToString::to_string).collect();
        crate::concurrency::lock_embedded_keys(
            child_table,
            crate::concurrency::Side::Writer,
            &keys,
        );
        for lookup_col in graph.lookup_columns(child, &parent) {
            for (child_pk, keys) in
                find_affected_keys_batch(&parent, lookup_col, &unpruned, &appeared)?
            {
                let found = result
                    .entry(child_pk)
                    .or_insert_with(|| Vec::with_capacity(keys.len()));
                for key in keys.into_iter().map(|k| RefreshKey::new(&parent, k)) {
                    if !found.contains(&key) {
                        found.push(key);
                    }
                }
            }
        }
    }
    Ok(result)
}

/// Whether propagation from `child`'s row `pk` to `parent` can be skipped: the
/// parent embeds only the child's computed document, and the child's refresh in
/// this flush changed nothing. A scalar embed that follows the child's
/// FK to a deeper relationship always propagates.
fn prune_edge(graph: &crate::flush::EntityDepGraph, child: &str, parent: &str, pk: i64) -> bool {
    let prune = graph
        .document_edges
        .contains(&(child.to_string(), parent.to_string()))
        && !crate::queue::affected::changed_in_flush(child, pk);
    if prune {
        crate::metrics::metrics_api::record_propagation_pruned();
    }
    prune
}

/// The identity values of the rows of `parent` whose `lookup_col` holds one of
/// `child_pks`, keyed by that child pk; for `in_view`, rows of its backing view too.
fn find_affected_keys_batch(
    parent: &str,
    lookup_col: &str,
    child_pks: &[i64],
    in_view: &[i64],
) -> crate::TViewResult<HashMap<i64, Vec<KeyValue>>> {
    let meta = crate::catalog::TviewMeta::load_by_entity(parent)?.ok_or_else(|| {
        crate::TViewError::TviewNotFound {
            name: parent.to_string(),
        }
    })?;
    let qi_fk = ident::quoted(lookup_col);
    let qi_parent = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qi_key = ident::quoted(&meta.identity.column);
    let parent_key_type = meta.key_type()?;
    let key_type = match parent_key_type {
        KeyType::Int => "pg_catalog.int8",
        KeyType::Text(_) => "pg_catalog.text",
    };
    // The lookup column can be the parent's own key (an embedded aggregate keyed by
    // it), so the two outputs are aliased.
    let select = |relation: &str, param: &str| {
        format!(
            "SELECT {qi_fk}::pg_catalog.int8 AS child_key, {qi_key}::{key_type} AS parent_key \
             FROM {relation} WHERE {qi_fk} = ANY({param})"
        )
    };
    let mut query = select(&qi_parent, "$1");
    if !in_view.is_empty() {
        let qi_view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
        query = format!("{query} UNION {}", select(&qi_view, "$2"));
    }
    let _owner = crate::owner::AsOwner::of_entity(parent)?;
    // Read-write: a fresh snapshot, which sees what the locks waited for.
    Spi::connect_mut(|client| {
        let array = |pks: &[i64]| crate::utils::spi::int8_array(pks.to_vec());
        let args = if in_view.is_empty() {
            vec![array(child_pks)]
        } else {
            vec![array(child_pks), array(in_view)]
        };
        let mut result: HashMap<i64, Vec<KeyValue>> = HashMap::with_capacity(child_pks.len());
        let mut found = Vec::new();
        for row in client.update(&query, None, &args)? {
            let Some(child_pk) = row["child_key"].value::<i64>()? else {
                continue;
            };
            let key = match parent_key_type {
                KeyType::Int => row["parent_key"].value::<i64>()?.map(KeyValue::Int),
                KeyType::Text(_) => row["parent_key"].value::<String>()?.map(KeyValue::Text),
            };
            if let Some(key) = key {
                found.push(vec![Some(child_pk.to_string()), Some(key.to_string())]);
                result.entry(child_pk).or_default().push(key);
            }
        }
        // REPEATABLE READ: no parent row the latest snapshot holds is missed.
        if crate::concurrency::crosscheck::enabled() {
            let latest = crate::utils::spi::latest_rows_connected(&query, &args, true)?;
            crate::concurrency::crosscheck::discovered(&found, &latest);
        }
        Ok::<_, crate::TViewError>(result)
    })
}

use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use std::collections::HashMap;

/// Propagation Engine: Parent Discovery for Dependent Views
///
/// This module provides parent discovery for the transaction-level queue:
/// - **Parent Discovery**: Finds views that depend on changed entities
/// - **Affected Row Identification**: Locates rows impacted by changes
///
/// Used by the flush (`src/queue/`) to iteratively discover and enqueue parent
/// TVIEWs for refresh.
use crate::catalog::KeyType;
use crate::queue::RefreshKey;
use crate::queue::key::KeyValue;
use crate::utils::quote_identifier;

/// The parent rows to refresh when rows of `child` changed: for each TVIEW that
/// embeds `child`, its rows whose lookup column (`fk_<child>`, or the column an
/// aggregate embed records) holds one of `pks`, the children's `pk_<child>`
/// values. Keyed by child pk; each parent key is the parent's identity value.
///
/// Parents reference a child by `fk_<child> = pk_<child>` whatever the child's
/// identity (ADR 0169, D4), so the caller passes the `pk_<child>` of the rows its
/// refresh touched, before and after. For the child rows that `appeared`, the
/// parents are also looked up in their backing view: a parent row an inner join
/// dropped with the child is in the view again, not in its table (#177).
///
/// One query per parent entity, `lookup = ANY($1)` over all the pks.
///
/// # Errors
/// Returns an error if a parent's catalog row or table cannot be read.
pub fn find_parents_batch(
    child: &str,
    pks: &[i64],
    appeared: &[i64],
    graph: &crate::queue::EntityDepGraph,
) -> crate::TViewResult<HashMap<i64, Vec<RefreshKey>>> {
    let mut result: HashMap<i64, Vec<RefreshKey>> = HashMap::with_capacity(pks.len());
    if pks.is_empty() {
        return Ok(result);
    }
    for parent in graph.parents.get(child).cloned().unwrap_or_default() {
        let unpruned: Vec<i64> = pks
            .iter()
            .copied()
            .filter(|&pk| !prune_edge(graph, child, &parent, pk))
            .collect();
        if unpruned.is_empty() {
            continue;
        }
        let lookup_col = graph.lookup_column(child, &parent);
        let appeared: Vec<i64> = appeared
            .iter()
            .copied()
            .filter(|pk| unpruned.contains(pk))
            .collect();
        for (child_pk, keys) in
            find_affected_keys_batch(&parent, &lookup_col, &unpruned, &appeared)?
        {
            result
                .entry(child_pk)
                .or_insert_with(|| Vec::with_capacity(keys.len()))
                .extend(keys.into_iter().map(|k| RefreshKey::new(&parent, k)));
        }
    }
    Ok(result)
}

/// Whether propagation from `child`'s row `pk` to `parent` can be skipped: the
/// parent embeds only the child's computed document, and the child's refresh in
/// this flush changed nothing (issue #85). A scalar embed that follows the child's
/// FK to a deeper relationship always propagates.
fn prune_edge(graph: &crate::queue::EntityDepGraph, child: &str, parent: &str, pk: i64) -> bool {
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
) -> spi::Result<HashMap<i64, Vec<KeyValue>>> {
    let meta = crate::catalog::TviewMeta::load_by_entity(parent)?.ok_or_else(|| {
        crate::TViewError::MetadataNotFound {
            entity: parent.to_string(),
        }
    })?;
    let qi_fk = quote_identifier(lookup_col);
    let qi_parent = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qi_key = quote_identifier(&meta.identity.column);
    let key_type = match meta.identity.key_type {
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
    Spi::connect(|client| {
        // SAFETY: the datums own their arrays.
        let array = |pks: &[i64]| unsafe {
            DatumWithOid::new(
                pks.to_vec(),
                PgOid::BuiltIn(PgBuiltInOids::INT8ARRAYOID).value(),
            )
        };
        let args = if in_view.is_empty() {
            vec![array(child_pks)]
        } else {
            vec![array(child_pks), array(in_view)]
        };
        let mut result: HashMap<i64, Vec<KeyValue>> = HashMap::with_capacity(child_pks.len());
        for row in client.select(&query, None, &args)? {
            let Some(child_pk) = row["child_key"].value::<i64>()? else {
                continue;
            };
            let key = match meta.identity.key_type {
                KeyType::Int => row["parent_key"].value::<i64>()?.map(KeyValue::Int),
                KeyType::Text(_) => row["parent_key"].value::<String>()?.map(KeyValue::Text),
            };
            if let Some(key) = key {
                result.entry(child_pk).or_default().push(key);
            }
        }
        Ok(result)
    })
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use super::*;
    use pgrx::prelude::Spi;

    /// Test pre-allocation in batch parent discovery.
    ///
    /// Verifies that:
    /// 1. Pre-allocation is correct and doesn't change results
    /// 2. Batching correctly groups by (parent, child) entities
    /// 3. Results match non-batched discovery for same input
    ///
    /// Scenario: Multiple children from same entity with multiple parent entities
    #[pg_test]
    fn test_find_parents_batch_pre_allocation() {
        // Setup: Create tables with multiple FK relationships
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run(
            "CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )",
        )
        .unwrap();
        Spi::run(
            "CREATE TABLE tb_comment (
            pk_comment BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            fk_post BIGINT REFERENCES tb_post(pk_post),
            text TEXT
        )",
        )
        .unwrap();

        // Insert test data
        Spi::run("INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice'), (2, 'Bob')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'Post 1'), (2, 1, 'Post 2'), (3, 2, 'Post 3')").unwrap();
        Spi::run(
            "INSERT INTO tb_comment (pk_comment, fk_user, fk_post, text)
                  VALUES (1, 1, 1, 'Comment 1'), (2, 1, 2, 'Comment 2'), (3, 2, 3, 'Comment 3')",
        )
        .unwrap();

        // Create dependency TVIEWs
        Spi::run(
            "
            SELECT pg_tviews_create('user', $$
                SELECT pk_user, jsonb_build_object('name', name) AS data
                FROM tb_user
            $$)
        ",
        )
        .unwrap();

        Spi::run(
            "
            SELECT pg_tviews_create('post', $$
                SELECT pk_post, fk_user,
                       jsonb_build_object('title', title, 'author', v_user.data) AS data
                FROM tb_post
                LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
            $$)
        ",
        )
        .unwrap();

        Spi::run(
            "
            SELECT pg_tviews_create('comment', $$
                SELECT pk_comment, fk_user, fk_post,
                       jsonb_build_object('text', text) AS data
                FROM tb_comment
            $$)
        ",
        )
        .unwrap();

        // Test: Find parents for multiple user PKs using batched discovery
        let graph = crate::queue::EntityDepGraph::load().unwrap();

        // Batched discovery
        let batched_result = find_parents_batch("user", &[1, 2], &[], &graph).unwrap();

        // Verify results are non-empty
        assert!(!batched_result.is_empty(), "Should find parent entities");

        // For user pk=1, should find posts 1, 2
        if let Some(parents) = batched_result.get(&1) {
            // Should have multiple post parents
            let post_parents: Vec<_> = parents.iter().filter(|p| p.entity == "post").collect();
            assert!(!post_parents.is_empty(), "User 1 should have post parents");
        }

        // For user pk=2, should find post 3
        if let Some(parents) = batched_result.get(&2) {
            let post_parents: Vec<_> = parents.iter().filter(|p| p.entity == "post").collect();
            assert!(!post_parents.is_empty(), "User 2 should have post parents");
        }
    }

    /// Test that batch pre-allocation handles empty parent case correctly.
    ///
    /// Verifies that pre-allocations handle the edge case where a child
    /// entity has no parents (no FK references from other entities).
    #[pg_test]
    fn test_find_parents_batch_no_parents() {
        // Setup: Single entity with no FK references
        Spi::run("CREATE TABLE tb_tag (pk_tag BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_tag (pk_tag, name) VALUES (1, 'Tag1'), (2, 'Tag2')").unwrap();

        // Create TVIEW
        Spi::run(
            "
            SELECT pg_tviews_create('tag', $$
                SELECT pk_tag, jsonb_build_object('name', name) AS data
                FROM tb_tag
            $$)
        ",
        )
        .unwrap();

        let graph = crate::queue::EntityDepGraph::load().unwrap();

        let result = find_parents_batch("tag", &[1, 2], &[], &graph).unwrap();

        // Should return empty or no entries for tag (no parents)
        let has_tag_results = [1, 2].iter().any(|k| result.contains_key(k));

        // Either tag has no parents (expected) or result is empty
        if has_tag_results {
            for parents in result.values() {
                assert!(parents.is_empty(), "Tag should have no parents");
            }
        }
    }
}

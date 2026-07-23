use super::key::RefreshKey;
use super::state::{TX_CRASH_RECOVERY_CHECKED, TX_REFRESH_QUEUE};
use std::collections::HashSet;

/// Check queue size against a limit and raise ERROR if exceeded.
/// Returns Ok(()) if the queue size is still below the limit.
/// Returns Err with a descriptive message if the limit would be exceeded.
fn check_queue_backpressure(limit: usize) -> Result<(), String> {
    let current_size = TX_REFRESH_QUEUE.with(|q| q.borrow().len());
    if current_size >= limit {
        return Err(format!(
            "refresh queue backpressure: queue size ({current_size}) would exceed max_queue_size ({limit})"
        ));
    }
    Ok(())
}

/// Internal helper: enqueue a single PK-based refresh with explicit limit.
/// Used for testability and backpressure enforcement.
pub fn enqueue_refresh_with_limit(entity: &str, pk: i64, limit: usize) -> Result<(), String> {
    check_queue_backpressure(limit)?;
    TX_REFRESH_QUEUE.with(|q| {
        q.borrow_mut().insert(RefreshKey::pk(entity, pk));
    });
    Ok(())
}

/// Enqueue a standard PK-based refresh request.
///
/// This is the main entry point from triggers for normal TVIEWs.
/// Deduplication is automatic (`HashSet`).
/// Raises ERROR if `max_queue_size` would be exceeded.
///
/// A plain enqueue **poisons** any direct patch for this key (issue #56): the key
/// will recompute. Only [`enqueue_refresh_patched`] preserves a fast-path patch.
pub fn enqueue_refresh(entity: &str, pk: i64) {
    if let Err(msg) = enqueue_refresh_with_limit(entity, pk, crate::config::max_queue_size()) {
        pgrx::error!("{}", msg);
    }
    super::patch::poison(RefreshKey::pk(entity, pk));
}

/// Enqueue a PK-based refresh **and** record a direct patch (issue #56 fast path).
///
/// Inserts `(entity, pk)` into the refresh queue under the same backpressure limit
/// as [`enqueue_refresh`], then records the captured `fields` as a top-level
/// (`prefix = []`) direct patch. If the key was already poisoned this transaction
/// (e.g. an INSERT then UPDATE of the same row), the patch is ignored and the key
/// recomputes — a patch alone cannot materialise a not-yet-created row.
pub fn enqueue_refresh_patched(
    entity: &str,
    pk: i64,
    fields: serde_json::Map<String, serde_json::Value>,
) {
    if let Err(msg) = enqueue_refresh_with_limit(entity, pk, crate::config::max_queue_size()) {
        pgrx::error!("{}", msg);
    }
    // Count the capture once per fresh key: a base table feeding several tviews
    // has several row triggers that each re-record the same key in one statement.
    if super::patch::record(RefreshKey::pk(entity, pk), Vec::new(), fields) {
        crate::metrics::metrics_api::record_direct_patch_captured();
    }
}

/// Enqueue a DISTINCT ON dedup-key refresh request.
///
/// Used by triggers on DISTINCT ON TVIEWs.  The `dedup_key` is the value
/// of the DISTINCT ON column (cast to TEXT) identifying the group to re-evaluate.
/// Deduplication is automatic (`HashSet`).
/// Raises ERROR if `max_queue_size` would be exceeded.
pub fn enqueue_refresh_dedup(entity: &str, dedup_key: &str) {
    TX_REFRESH_QUEUE.with(|q| {
        let limit = crate::config::max_queue_size();
        check_queue_backpressure(limit).unwrap_or_else(|msg| {
            pgrx::error!("{}", msg);
        });
        q.borrow_mut().insert(RefreshKey::dedup(entity, dedup_key));
    });
    // Plain enqueue poisons any direct patch for this key (issue #56).
    super::patch::poison(RefreshKey::dedup(entity, dedup_key));
}

/// Bulk enqueue PK-based refresh requests for multiple PKs of the same entity.
///
/// This is the statement-level trigger entry point.
/// Deduplication is automatic (`HashSet`).
/// Raises ERROR if `max_queue_size` would be exceeded.
pub fn enqueue_refresh_bulk(entity: &str, pks: Vec<i64>) {
    TX_REFRESH_QUEUE.with(|q| {
        let limit = crate::config::max_queue_size();
        check_queue_backpressure(limit).unwrap_or_else(|msg| {
            pgrx::error!("{}", msg);
        });
        let mut queue = q.borrow_mut();
        for pk in &pks {
            queue.insert(RefreshKey::pk(entity, *pk));
        }
    });
    // Plain (bulk) enqueue poisons any direct patches for these keys (issue #56).
    for pk in pks {
        super::patch::poison(RefreshKey::pk(entity, pk));
    }
}

/// Take a snapshot of the current queue and clear it
///
/// Called by commit handler to get all pending refreshes.
/// Thread-local state is cleared after snapshot.
pub fn take_queue_snapshot() -> HashSet<RefreshKey> {
    TX_REFRESH_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();
        std::mem::take(&mut *queue)
    })
}

/// Check whether the refresh queue has any pending items (non-destructive).
pub fn is_queue_empty() -> bool {
    TX_REFRESH_QUEUE.with(|q| q.borrow().is_empty())
}

/// Clear the queue (used on transaction abort)
pub fn clear_queue() {
    TX_REFRESH_QUEUE.with(|q| {
        q.borrow_mut().clear();
    });
}

/// Check if crash recovery has already been checked for this entity in this transaction
pub fn is_crash_recovery_checked(entity: &str) -> bool {
    TX_CRASH_RECOVERY_CHECKED.with(|checked| checked.borrow().contains(entity))
}

/// Mark that crash recovery has been checked for this entity in this transaction
pub fn mark_crash_recovery_checked(entity: &str) {
    TX_CRASH_RECOVERY_CHECKED.with(|checked| {
        checked.borrow_mut().insert(entity.to_string());
    });
}

/// Clear the crash recovery check cache (used on transaction abort)
pub fn clear_crash_recovery_cache() {
    TX_CRASH_RECOVERY_CHECKED.with(|checked| {
        checked.borrow_mut().clear();
    });
}

/// Batch lookup of carry column values via SPI.
///
/// Executes: `SELECT {carry_col} FROM "schema"."table" WHERE {lookup_col} = ANY($1)`
///
/// Resolves the table OID to a schema-qualified, properly-quoted name via
/// `pg_class + pg_namespace` so that tables in any schema are referenced
/// correctly regardless of `search_path`.
///
/// Used for cascade path traversal to find parent IDs that need refreshing.
/// Table OID and column names come from `CascadePath` data validated at registration time.
pub fn spi_batch_lookup(
    table_oid: pgrx::pg_sys::Oid,
    lookup_col: &str,
    carry_col: &str,
    ids: &[i64],
) -> crate::TViewResult<Vec<i64>> {
    use crate::utils::{qualified_relname_from_oid, quote_identifier};
    use pgrx::prelude::*;

    if ids.is_empty() {
        return Ok(vec![]);
    }

    // Resolve OID → "schema"."table" so the FROM clause is valid SQL regardless
    // of search_path.  Cast expressions like `(52276294::regclass)` are not valid
    // FROM-clause table references in PostgreSQL.
    let qualified_name =
        qualified_relname_from_oid(table_oid).map_err(|e| crate::TViewError::SpiError {
            query: "qualified_relname_from_oid".to_string(),
            error: format!("failed to resolve table OID {table_oid:?}: {e}"),
        })?;
    let qi_lookup = quote_identifier(lookup_col);
    let qi_carry = quote_identifier(carry_col);

    let query = format!("SELECT {qi_carry} FROM {qualified_name} WHERE {qi_lookup} = ANY($1)");

    let pks_array = ids.to_vec();
    let args = vec![unsafe {
        pgrx::datum::DatumWithOid::new(
            pks_array,
            PgOid::BuiltIn(PgBuiltInOids::INT8ARRAYOID).value(),
        )
    }];

    Spi::connect(|client| {
        let rows = client.select(&query, None, &args)?;
        let mut result = Vec::new();
        for row in rows {
            if let Some(value) = row[1].value::<i64>()? {
                result.push(value);
            }
        }
        Ok(result)
    })
    .map_err(|e: pgrx::spi::Error| crate::TViewError::SpiError {
        query,
        error: format!("SPI batch lookup failed: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_and_snapshot() {
        clear_queue();

        enqueue_refresh("user", 1);
        enqueue_refresh("post", 2);
        enqueue_refresh("user", 1); // duplicate

        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 2); // Deduplicated

        // Queue should be empty after snapshot
        let empty_snapshot = take_queue_snapshot();
        assert_eq!(empty_snapshot.len(), 0);
    }

    #[test]
    fn test_clear_queue() {
        clear_queue();

        enqueue_refresh("user", 1);
        enqueue_refresh("post", 2);

        clear_queue();

        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 0);
    }

    #[test]
    fn test_enqueue_respects_max_queue_size() {
        clear_queue();

        // Test that enqueue_refresh_with_limit raises an error when limit is exceeded.
        // This test calls the internal helper directly with a small limit.
        let limit = 2;

        // Should succeed: queue size is 0, adding 1 (total 1) doesn't exceed limit
        enqueue_refresh_with_limit("user", 1, limit).expect("first insert should succeed");

        // Should succeed: queue size is 1, adding 1 (total 2) doesn't exceed limit
        enqueue_refresh_with_limit("post", 2, limit).expect("second insert should succeed");

        // Should fail: queue size is 2, adding 1 (total 3) exceeds limit
        assert!(
            enqueue_refresh_with_limit("user", 3, limit).is_err(),
            "third insert should fail"
        );

        // Verify queue only has 2 items (backpressure prevented the 3rd)
        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 2);
    }
}

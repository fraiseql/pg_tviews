use super::ops::{clear_queue, take_queue_snapshot};
use crate::TViewResult;
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::collections::HashSet;
use std::os::raw::c_void;
use std::panic::AssertUnwindSafe;

// Thread-local storage for savepoint support
thread_local! {
    /// Current savepoint depth (0 = no savepoints)
    static SAVEPOINT_DEPTH: std::cell::RefCell<usize> = const { std::cell::RefCell::new(0) };

    /// Queue snapshots for each savepoint level
    static QUEUE_SNAPSHOTS: std::cell::RefCell<Vec<HashSet<super::key::RefreshKey>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Transaction event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XactEvent {
    Commit,
    Abort,
    PreCommit,
    Prepare, // XACT_EVENT_PREPARE
}

/// Register the transaction callback (called from enqueue logic)
///
/// This uses `PostgreSQL`'s `RegisterXactCallback` FFI to install our handler.
/// The callback will be invoked at transaction commit/abort.
pub unsafe fn register_xact_callback() {
    // Safety: We're calling into PostgreSQL FFI
    // The callback function must be extern "C" and #[no_mangle]

    unsafe {
        pg_sys::RegisterXactCallback(Some(tview_xact_callback), std::ptr::null_mut());
    }
}

/// Register the subtransaction callback for savepoint support
///
/// This uses `PostgreSQL`'s `RegisterSubXactCallback` FFI to handle savepoints.
/// The callback will be invoked when savepoints are created/released/rolled back.
pub unsafe fn register_subxact_callback() {
    // Safety: We're calling into PostgreSQL FFI
    // The callback function must be extern "C" and #[no_mangle]

    unsafe {
        pg_sys::RegisterSubXactCallback(Some(tview_subxact_callback), std::ptr::null_mut());
    }
}

/// Transaction callback handler (invoked by `PostgreSQL`)
///
/// This is called at transaction events (COMMIT, ABORT, etc.)
///
/// # Safety
/// This is an extern "C-unwind" callback invoked by `PostgreSQL` internals.
///
/// # Error handling
/// Errors from `handle_pre_commit`/`handle_prepare` are reported via pgrx's
/// `error!()` macro, which triggers `ereport(ERROR)` and longjmps out of
/// the callback.  `PostgreSQL` will then abort the transaction.
///
/// We intentionally avoid `catch_unwind` here: SPI operations in the
/// pre-commit handler may trigger `PostgreSQL` longjmps, and intercepting
/// those via `catch_unwind` corrupts `PG_exception_stack`, causing SIGABRT.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn tview_xact_callback(event: u32, _arg: *mut c_void) {
    // Map PostgreSQL XactEvent C enum to our Rust enum.
    // Use pg_sys constants to be version-safe.
    #[allow(non_upper_case_globals)] // Reason: pg_sys XactEvent constants use UPPER_CASE naming
    let xact_event = match event {
        pg_sys::XactEvent::XACT_EVENT_COMMIT => XactEvent::Commit,
        pg_sys::XactEvent::XACT_EVENT_PRE_COMMIT => XactEvent::PreCommit,
        pg_sys::XactEvent::XACT_EVENT_ABORT => XactEvent::Abort,
        pg_sys::XactEvent::XACT_EVENT_PREPARE => XactEvent::Prepare,
        _ => return, // Ignore PARALLEL_*, PRE_PREPARE, etc.
    };

    // Handle event.
    //
    // NOTE: SPI is NOT available during transaction callbacks (PRE_COMMIT, COMMIT, ABORT).
    // Executing SPI queries here crashes the server. Queue flush (which uses SPI) is
    // handled by the ProcessUtility hook intercepting COMMIT instead.
    match xact_event {
        XactEvent::PreCommit | XactEvent::Commit => {
            // Queue flush + audit flush happen in ProcessUtility hook before COMMIT.
            // Clear audit buffer as safety net (should already be empty after flush).
            crate::audit::clear_audit_buffer();
            crate::metrics::metrics_api::reset_metrics();
        }
        XactEvent::Prepare => {
            // PREPARE TRANSACTION also goes through ProcessUtility hook.
            crate::audit::clear_audit_buffer();
            crate::metrics::metrics_api::reset_metrics();
        }
        XactEvent::Abort => {
            clear_queue();
            crate::audit::clear_audit_buffer();
            crate::metrics::metrics_api::reset_metrics();
        }
    }
}

/// Subtransaction callback handler (invoked by `PostgreSQL` for savepoints)
///
/// This is called when savepoints are created, released, or rolled back to.
/// We need to maintain queue snapshots to properly handle ROLLBACK TO SAVEPOINT.
///
/// # Safety
/// This is an extern "C-unwind" callback invoked by `PostgreSQL` internals.
/// Must not panic or unwind.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn tview_subxact_callback(
    event: u32,
    _subxid: pg_sys::SubTransactionId,
    _parent_subid: pg_sys::SubTransactionId,
    _arg: *mut c_void,
) {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        match event {
            pg_sys::SubXactEvent::SUBXACT_EVENT_START_SUB => {
                // SAVEPOINT created: increment depth and snapshot current queue
                SAVEPOINT_DEPTH.with(|d| {
                    let mut depth = d.borrow_mut();
                    *depth += 1;
                });

                // Take snapshot of current queue state
                let snapshot = take_queue_snapshot();
                QUEUE_SNAPSHOTS.with(|s| {
                    s.borrow_mut().push(snapshot);
                });
            }
            pg_sys::SubXactEvent::SUBXACT_EVENT_ABORT_SUB => {
                // ROLLBACK TO SAVEPOINT: restore queue to snapshot
                decrement_savepoint_depth();

                // Restore queue from snapshot
                if let Some(snapshot) = QUEUE_SNAPSHOTS.with(|s| s.borrow_mut().pop()) {
                    // Replace current queue with the snapshot
                    super::state::replace_queue(snapshot);
                }
            }
            pg_sys::SubXactEvent::SUBXACT_EVENT_COMMIT_SUB => {
                // RELEASE SAVEPOINT: just decrement depth and discard snapshot
                decrement_savepoint_depth();

                // Discard the snapshot (savepoint committed)
                QUEUE_SNAPSHOTS.with(|s| {
                    s.borrow_mut().pop();
                });
            }
            _ => {
                // Ignore other subtransaction events
            }
        }
    }));

    if result.is_err() {
        // Non-fatal: savepoint tracking is defensive. Use warning instead of error
        // to avoid SIGABRT from panic_any in raw extern "C-unwind" context.
        warning!("PANIC in subtransaction callback - this is a bug!");
    }
}

/// Decrement `SAVEPOINT_DEPTH` with saturating subtraction.
///
/// Emits a warning if the depth is already 0, which indicates unexpected
/// event ordering (e.g., extension loaded mid-transaction).
fn decrement_savepoint_depth() {
    SAVEPOINT_DEPTH.with(|d| {
        let mut depth = d.borrow_mut();
        if *depth == 0 {
            warning!("pg_tviews: subxact depth underflow — event ordering unexpected");
        }
        *depth = depth.saturating_sub(1);
    });
}

/// Flush the refresh queue: process all pending TVIEW refreshes.
///
/// Called by the `ProcessUtility` hook when intercepting COMMIT, **before**
/// the actual commit begins. SPI must be available when this is called.
///
/// **Must NOT be called from transaction callbacks** (`PRE_COMMIT`, `COMMIT`, `ABORT`)
/// because `PostgreSQL` does not allow SPI queries during those callbacks.
///
/// This implementation correctly handles propagation by using a local queue
/// for discovered parent refreshes. The workflow:
///
/// 1. Take initial snapshot from triggers (from triggers)
/// 2. Process in dependency order (children before parents)
/// 3. Discover parent refreshes during processing
/// 4. Add parents to local pending queue
/// 5. Repeat until no more refreshes discovered (fixpoint)
///
/// # Correctness
///
/// - Each (entity, pk) processed exactly once (tracked in `processed` set)
/// - Dependency order respected (topological sort per iteration)
/// - Propagation coalesced (parents discovered during refresh added to queue)
/// - Transaction-safe (fail-fast aborts transaction on first error)
pub fn flush_refresh_queue() -> TViewResult<()> {
    // Take initial snapshot from triggers
    let mut pending = take_queue_snapshot();

    if pending.is_empty() {
        return Ok(());
    }

    // Start timing the entire refresh operation
    let refresh_timer = crate::metrics::metrics_api::record_refresh_start();

    // Load dependency graph once (cached)
    let graph = super::cache::graph_cache::load_cached()?;

    // Track processed keys to avoid duplicates
    // Pre-allocate with capacity based on initial pending size
    let mut processed: std::collections::HashSet<super::key::RefreshKey> =
        std::collections::HashSet::with_capacity(pending.len().max(16));

    // Outer drain loop: after the inner loop empties `pending`, check for
    // late-enqueued items from triggers that fired during refresh (e.g.,
    // pg_treekey cascading child rows in tb_location).  The `processed` set
    // carries across drain passes so already-refreshed keys are not repeated.
    let mut iteration = 1;
    loop {
        // Inner loop: process pending until empty (propagation via parents)
        while !pending.is_empty() {
            // Sort this batch by dependency order
            let sorted_keys = graph.sort_keys(pending.drain().collect());

            // Group keys by entity for bulk refresh
            // Pre-allocate with estimated entity count (typically 3-10 entities)
            let mut keys_by_entity: std::collections::HashMap<String, Vec<super::key::RefreshKey>> =
                std::collections::HashMap::with_capacity(8);

            for key in sorted_keys {
                // Skip if already processed (deduplication)
                if !processed.insert(key.clone()) {
                    continue;
                }
                keys_by_entity
                    .entry(key.entity.clone())
                    .or_default()
                    .push(key);
            }

            // Process each entity group
            for (entity, entity_keys) in keys_by_entity {
                if entity_keys.len() == 1 {
                    // Single key: use existing individual refresh
                    let key = &entity_keys[0];
                    let parents = refresh_and_get_parents(key, &graph)?;

                    // Add discovered parents to pending queue
                    for parent_key in parents {
                        if !processed.contains(&parent_key) {
                            pending.insert(parent_key);
                        }
                    }
                } else {
                    // Multiple keys for same entity: use bulk refresh (PK-only path)
                    // Pre-allocate Vec based on PK-only keys (exclude dedup keys)
                    let mut pks =
                        Vec::with_capacity(entity_keys.iter().filter(|k| !k.is_dedup()).count());
                    for key in &entity_keys {
                        if !key.is_dedup() {
                            pks.push(key.pk);
                        }
                    }

                    // Bulk refresh this entity
                    // FAIL-FAST: Propagate error immediately to abort transaction
                    crate::refresh::refresh_bulk(&entity, &pks)?;

                    // Discover parents for all keys in this entity group (P-07: batched)
                    // Instead of N × M separate queries, issue M batched queries (one per parent entity)
                    let parent_map = crate::propagate::find_parents_batch(&entity_keys, &graph)?;

                    // Add discovered parents to pending queue
                    for parent_keys in parent_map.values() {
                        for parent_key in parent_keys {
                            if !processed.contains(parent_key) {
                                pending.insert(parent_key.clone());
                            }
                        }
                    }
                }
            }

            iteration += 1;

            // Safety check: prevent infinite loops
            let max_depth = crate::config::max_propagation_depth();
            if iteration > max_depth {
                return Err(crate::TViewError::PropagationDepthExceeded {
                    max_depth,
                    processed: processed.len(),
                });
            }
        }

        // Drain any items enqueued by triggers that fired during refresh
        let late = take_queue_snapshot();
        if late.is_empty() {
            break;
        }
        pending = late;
    }

    // Buffer batched audit entries: one per entity with aggregated row count.
    // Actual INSERT happens in flush_audit_buffer() called from the COMMIT hook.
    {
        let mut entity_counts: std::collections::HashMap<&str, i64> =
            std::collections::HashMap::new();
        for key in &processed {
            *entity_counts.entry(&key.entity).or_insert(0) += 1;
        }
        for (entity, count) in entity_counts {
            crate::audit::log_refresh(entity, count);
        }
    }

    // Record metrics
    crate::metrics::metrics_api::record_refresh_complete(
        processed.len(),
        iteration - 1,
        &refresh_timer,
    );

    Ok(())
}

/// Refresh a single entity+pk and return discovered parent keys (without refreshing them)
///
/// This function:
/// 1. Refreshes the given (entity, pk) using existing refresh logic
/// 2. Discovers parent entities that depend on this one
/// 3. Returns parent keys WITHOUT refreshing them (defer to queue)
///
/// # Design Note
///
/// This function returns parent keys for queue processing instead of
/// calling `refresh_pk()` recursively.
fn refresh_and_get_parents(
    key: &super::key::RefreshKey,
    graph: &super::EntityDepGraph,
) -> TViewResult<Vec<super::key::RefreshKey>> {
    // Load metadata
    use crate::catalog::TviewMeta;
    let meta = TviewMeta::load_by_entity(&key.entity)?.ok_or_else(|| {
        crate::TViewError::MetadataNotFound {
            entity: key.entity.clone(),
        }
    })?;

    // Refresh this entity — dispatch on key type
    if let Some(dedup) = &key.dedup_key {
        crate::refresh::refresh_by_dedup_key(meta.view_oid, dedup)?;
    } else {
        crate::refresh::refresh_pk(meta.view_oid, key.pk)?;
    }

    // Find parent entities (NEW: returns keys instead of refreshing)
    let parent_keys = crate::propagate::find_parents_for(key, graph)?;

    Ok(parent_keys)
}

// NOTE: Full 2PC support (PREPARE TRANSACTION with queue persistence) is not
// implemented in 0.1.0. The ProcessUtility hook rejects PREPARE TRANSACTION
// when TVIEW refreshes are pending. See hooks.rs for the guard.

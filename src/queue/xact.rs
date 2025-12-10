use pgrx::prelude::*;
use pgrx::pg_sys;
use std::os::raw::c_void;
use std::collections::HashSet;
use super::ops::{take_queue_snapshot, clear_queue, reset_scheduled_flag};
use crate::TViewResult;

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
    Prepare,  // XACT_EVENT_PREPARE
}

/// Register the transaction callback (called from enqueue logic)
///
/// This uses PostgreSQL's RegisterXactCallback FFI to install our handler.
/// The callback will be invoked at transaction commit/abort.
pub unsafe fn register_xact_callback() -> TViewResult<()> {
    // Safety: We're calling into PostgreSQL FFI
    // The callback function must be extern "C" and #[no_mangle]

    unsafe {
        pg_sys::RegisterXactCallback(
            Some(tview_xact_callback),
            std::ptr::null_mut(),
        );

        // Phase 9D: Register start-of-transaction callback for connection pooling safety
        pg_sys::RegisterXactCallback(
            Some(tview_xact_start_callback),
            std::ptr::null_mut(),
        );
    }

    Ok(())
}

/// Register the subtransaction callback for savepoint support
///
/// This uses PostgreSQL's RegisterSubXactCallback FFI to handle savepoints.
/// The callback will be invoked when savepoints are created/released/rolled back.
pub unsafe fn register_subxact_callback() -> TViewResult<()> {
    // Safety: We're calling into PostgreSQL FFI
    // The callback function must be extern "C" and #[no_mangle]

    unsafe {
        pg_sys::RegisterSubXactCallback(
            Some(tview_subxact_callback),
            std::ptr::null_mut(),
        );
    }

    Ok(())
}

/// Transaction callback handler (invoked by PostgreSQL)
///
/// This is called at transaction events (COMMIT, ABORT, etc.)
///
/// # Safety
/// This is an extern "C" callback invoked by PostgreSQL internals.
/// Must not panic or unwind.
#[no_mangle]
unsafe extern "C" fn tview_xact_callback(event: u32, _arg: *mut c_void) {
    // Determine event type (using PostgreSQL C API constants)
    let xact_event = match event {
        0 => XactEvent::Commit,      // XACT_EVENT_COMMIT
        1 => XactEvent::PreCommit,   // XACT_EVENT_PRE_COMMIT
        2 => XactEvent::Abort,       // XACT_EVENT_ABORT
        4 => XactEvent::Prepare,     // XACT_EVENT_PREPARE
        _ => return, // Ignore other events
    };

    // Handle event
    match xact_event {
        XactEvent::PreCommit => {
            // PRE_COMMIT: Flush queue before transaction commits
            // This is the main refresh point
            //
            // CRITICAL: We must propagate errors to abort the transaction.
            // Per PRD R2: "If refresh fails: the entire transaction fails and rolls back."
            //
            // PostgreSQL behavior:
            // - If this callback returns normally → transaction commits
            // - If this callback returns error!() or panics → transaction aborts
            //
            // We MUST NOT catch errors here - let them propagate to PostgreSQL
            if let Err(e) = handle_pre_commit() {
                // Use pgrx error!() macro to abort transaction
                error!("TVIEW refresh failed during PRE_COMMIT, aborting transaction: {:?}", e);
                // This will never return - PostgreSQL longjmps to abort handler
            }
        }
        XactEvent::Prepare => {
            // PREPARE: Serialize queue to persistent storage
            // This ensures 2PC transactions don't lose pending refreshes
            if let Err(e) = handle_prepare() {
                error!("TVIEW failed to persist queue during PREPARE: {:?}", e);
                // For PREPARE, we should abort the prepare operation
                // PostgreSQL will handle this by failing the PREPARE TRANSACTION
            }
        }
        XactEvent::Abort => {
            // ABORT: Clear queue without refreshing
            clear_queue();
            reset_scheduled_flag();
            // Reset metrics for aborted transaction
            crate::metrics::metrics_api::reset_metrics();
        }
        XactEvent::Commit => {
            // COMMIT: Cleanup (queue already flushed in PRE_COMMIT)
            reset_scheduled_flag();
            // Reset metrics for completed transaction
            crate::metrics::metrics_api::reset_metrics();
        }
    }
}

/// Start-of-transaction callback for connection pooling safety (Phase 9D)
///
/// This ensures thread-local state is cleared at the start of each transaction,
/// preventing queue leakage between transactions in connection poolers like PgBouncer.
///
/// # Safety
/// This is an extern "C" callback invoked by PostgreSQL internals.
/// Must not panic or unwind.
#[no_mangle]
unsafe extern "C" fn tview_xact_start_callback(event: u32, _arg: *mut c_void) {
    if event == 3 { // XACT_EVENT_START
        // Defensive: Clear any leftover state from previous transaction
        // This prevents queue leakage in connection poolers (PgBouncer, etc.)
        clear_queue();
        reset_scheduled_flag();
        info!("TVIEW: Transaction started, cleared thread-local state for connection pooling safety");
    }
}

/// Subtransaction callback handler (invoked by PostgreSQL for savepoints)
///
/// This is called when savepoints are created, released, or rolled back to.
/// We need to maintain queue snapshots to properly handle ROLLBACK TO SAVEPOINT.
///
/// # Safety
/// This is an extern "C" callback invoked by PostgreSQL internals.
/// Must not panic or unwind.
#[no_mangle]
unsafe extern "C" fn tview_subxact_callback(
    event: u32,
    _subxid: pg_sys::SubTransactionId,
    _parent_subid: pg_sys::SubTransactionId,
    _arg: *mut c_void,
) {
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

            info!("TVIEW: Savepoint created (depth: {})", SAVEPOINT_DEPTH.with(|d| *d.borrow()));
        }
        pg_sys::SubXactEvent::SUBXACT_EVENT_ABORT_SUB => {
            // ROLLBACK TO SAVEPOINT: restore queue to snapshot
            SAVEPOINT_DEPTH.with(|d| {
                let mut depth = d.borrow_mut();
                *depth -= 1;
            });

            // Restore queue from snapshot
            if let Some(snapshot) = QUEUE_SNAPSHOTS.with(|s| s.borrow_mut().pop()) {
                // Replace current queue with the snapshot
                super::state::replace_queue(snapshot);
                info!("TVIEW: Savepoint rolled back (depth: {})", SAVEPOINT_DEPTH.with(|d| *d.borrow()));
            }
        }
        pg_sys::SubXactEvent::SUBXACT_EVENT_COMMIT_SUB => {
            // RELEASE SAVEPOINT: just decrement depth and discard snapshot
            SAVEPOINT_DEPTH.with(|d| {
                let mut depth = d.borrow_mut();
                *depth -= 1;
            });

            // Discard the snapshot (savepoint committed)
            QUEUE_SNAPSHOTS.with(|s| {
                s.borrow_mut().pop();
            });

            info!("TVIEW: Savepoint released (depth: {})", SAVEPOINT_DEPTH.with(|d| *d.borrow()));
        }
        _ => {
            // Ignore other subtransaction events
        }
    }
}

/// Handle PRE_COMMIT event: flush the queue and refresh TVIEWs
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
fn handle_pre_commit() -> TViewResult<()> {
    // Take initial snapshot from triggers
    let mut pending = take_queue_snapshot();

    if pending.is_empty() {
        return Ok(());
    }

    info!("TVIEW: Flushing {} initial refresh requests at commit", pending.len());

    // Start timing the entire refresh operation
    let refresh_timer = crate::metrics::metrics_api::record_refresh_start();

    // Load dependency graph once (cached)
    let graph = super::cache::graph_cache::load_cached()?;

    // Track processed keys to avoid duplicates
    let mut processed: std::collections::HashSet<super::key::RefreshKey> = std::collections::HashSet::new();

    // Process queue until empty (handles propagation)
    let mut iteration = 1;
    while !pending.is_empty() {
        // Sort this batch by dependency order
        let sorted_keys = graph.sort_keys(pending.drain().collect());

        info!("TVIEW: Processing iteration {}: {} refreshes", iteration, sorted_keys.len());

        // Group keys by entity for bulk refresh (Phase 9B optimization)
        let mut keys_by_entity: std::collections::HashMap<String, Vec<super::key::RefreshKey>> =
            std::collections::HashMap::new();

        for key in sorted_keys {
            // Skip if already processed (deduplication)
            if !processed.insert(key.clone()) {
                continue;
            }
            keys_by_entity.entry(key.entity.clone()).or_default().push(key);
        }

        // Process each entity group
        for (entity, entity_keys) in keys_by_entity {
            if entity_keys.len() == 1 {
                // Single key: use existing individual refresh
                let key = &entity_keys[0];
                let parents = refresh_and_get_parents(key)?;

                // Add discovered parents to pending queue
                for parent_key in parents {
                    if !processed.contains(&parent_key) {
                        pending.insert(parent_key);
                    }
                }
            } else {
                // Multiple keys for same entity: use bulk refresh (Phase 9B)
                let pks: Vec<i64> = entity_keys.iter().map(|k| k.pk).collect();

                info!("TVIEW: Bulk refreshing entity '{}' with {} keys", entity, pks.len());

                // Bulk refresh this entity
                // FAIL-FAST: Propagate error immediately to abort transaction
                crate::refresh::refresh_bulk(&entity, pks)?;

                // Discover parents for all keys in this entity group
                for key in &entity_keys {
                    let parents = crate::propagate::find_parents_for(key)?;

                    // Add discovered parents to pending queue
                    for parent_key in parents {
                        if !processed.contains(&parent_key) {
                            pending.insert(parent_key);
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

    info!("TVIEW: Completed {} refresh operations in {} iterations", processed.len(), iteration - 1);

    // Record metrics
    crate::metrics::metrics_api::record_refresh_complete(
        processed.len(),
        iteration - 1,
        refresh_timer,
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
/// # Difference from Phase 1-5
///
/// Phase 1-5: `propagate_from_row()` called `refresh_pk()` recursively ❌
/// Phase 6D: Return parent keys for queue processing ✅
fn refresh_and_get_parents(key: &super::key::RefreshKey) -> TViewResult<Vec<super::key::RefreshKey>> {
    // Load metadata
    use crate::catalog::TviewMeta;
    let meta = TviewMeta::load_by_entity(&key.entity)?
        .ok_or_else(|| crate::TViewError::MetadataNotFound {
            entity: key.entity.clone(),
        })?;

    // Refresh this entity (existing logic)
    crate::refresh::refresh_pk(meta.view_oid, key.pk)?;

    // Find parent entities (NEW: returns keys instead of refreshing)
    let parent_keys = crate::propagate::find_parents_for(key)?;

    Ok(parent_keys)
}

/// Handle PREPARE TRANSACTION event: persist queue to database
///
/// This ensures that 2PC transactions don't lose pending refreshes.
/// The queue is serialized and stored in pg_tview_pending_refreshes.
fn handle_prepare() -> TViewResult<()> {
    // Get global transaction ID (GID) captured by ProcessUtility hook
    let gid = get_prepared_transaction_id()?;

    // Take snapshot of current queue
    let queue = take_queue_snapshot();

    if queue.is_empty() {
        // No refreshes pending, nothing to persist
        return Ok(());
    }

    info!("TVIEW: Persisting {} refresh requests for prepared transaction '{}'",
          queue.len(), gid);

    // Serialize queue using JSONB format (configurable in future)
    let serialized = super::persistence::SerializedQueue::from_queue(queue);
    let queue_size = serialized.keys.len() as i32;
    let queue_jsonb = serialized.into_jsonb()?;

    // Store in persistent table
    Spi::run_with_args(
        "INSERT INTO pg_tview_pending_refreshes
         (gid, refresh_queue, queue_size, expires_at)
         VALUES ($1, $2, $3, now() + interval '24 hours')",
        Some(vec![
            (PgOid::BuiltIn(PgBuiltInOids::TEXTOID), gid.clone().into_datum()),
            (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), queue_jsonb.into_datum()),
            (PgOid::BuiltIn(PgBuiltInOids::INT4OID), queue_size.into_datum()),
        ]),
    )?;

    // Clear in-memory queue (transaction is prepared, not committed)
    clear_queue();

    Ok(())
}

/// Get the global transaction ID for the currently preparing transaction
///
/// This retrieves the GID captured by the ProcessUtility hook during PREPARE TRANSACTION.
fn get_prepared_transaction_id() -> TViewResult<String> {
    crate::hooks::get_prepared_transaction_id()
}
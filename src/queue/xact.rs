use pgrx::prelude::*;
use pgrx::pg_sys;
use std::os::raw::c_void;
use super::ops::{take_queue_snapshot, clear_queue, reset_scheduled_flag};
use crate::TViewResult;

/// Transaction event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XactEvent {
    Commit,
    Abort,
    PreCommit,
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
            // - If this callback calls error!() or panics → transaction aborts
            //
            // We MUST NOT catch errors here - let them propagate to PostgreSQL
            if let Err(e) = handle_pre_commit() {
                // Use pgrx error!() macro to abort transaction
                error!("TVIEW refresh failed during PRE_COMMIT, aborting transaction: {:?}", e);
                // This will never return - PostgreSQL longjmps to abort handler
            }
        }
        XactEvent::Abort => {
            // ABORT: Clear queue without refreshing
            clear_queue();
            reset_scheduled_flag();
        }
        XactEvent::Commit => {
            // COMMIT: Cleanup (queue already flushed in PRE_COMMIT)
            reset_scheduled_flag();
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

    // Load dependency graph once (cached)
    let graph = super::graph::EntityDepGraph::load()?;

    // Track processed keys to avoid duplicates
    let mut processed: std::collections::HashSet<super::key::RefreshKey> = std::collections::HashSet::new();

    // Process queue until empty (handles propagation)
    let mut iteration = 1;
    while !pending.is_empty() {
        // Sort this batch by dependency order
        let sorted_keys = graph.sort_keys(pending.drain().collect());

        info!("TVIEW: Processing iteration {}: {} refreshes", iteration, sorted_keys.len());

        // Process each key in dependency order
        for key in sorted_keys {
            // Skip if already processed (deduplication)
            if !processed.insert(key.clone()) {
                continue;
            }

            // Refresh this entity and discover parents
            // FAIL-FAST: Propagate error immediately to abort transaction
            let parents = refresh_and_get_parents(&key)?;

            // Add discovered parents to pending queue
            for parent_key in parents {
                if !processed.contains(&parent_key) {
                    pending.insert(parent_key);
                }
            }
        }

        iteration += 1;

        // Safety check: prevent infinite loops
        if iteration > 100 {
            return Err(crate::TViewError::PropagationDepthExceeded {
                max_depth: 100,
                processed: processed.len(),
            });
        }
    }

    info!("TVIEW: Completed {} refresh operations in {} iterations", processed.len(), iteration - 1);

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
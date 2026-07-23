use super::ops::{
    clear_queue, is_crash_recovery_checked, mark_crash_recovery_checked, take_queue_snapshot,
};
use crate::TViewResult;
use pgrx::datum::DatumWithOid;
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

    /// Direct-patch map snapshots for each savepoint level (issue #56).
    /// Kept in lockstep with `QUEUE_SNAPSHOTS` so a patch rolls back exactly when
    /// its queue entry does.
    static PATCH_SNAPSHOTS: std::cell::RefCell<
        Vec<std::collections::HashMap<super::key::RefreshKey, super::patch::PatchState>>,
    > = const { std::cell::RefCell::new(Vec::new()) };
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
    // SAFETY: Called from PostgreSQL backend context. RegisterXactCallback
    // registers a valid extern "C" callback function pointer.
    unsafe {
        pg_sys::RegisterXactCallback(Some(tview_xact_callback), std::ptr::null_mut());
    }
}

/// Register the subtransaction callback for savepoint support
///
/// This uses `PostgreSQL`'s `RegisterSubXactCallback` FFI to handle savepoints.
/// The callback will be invoked when savepoints are created/released/rolled back.
pub unsafe fn register_subxact_callback() {
    // SAFETY: Called from PostgreSQL backend context. RegisterSubXactCallback
    // registers a valid extern "C" callback function pointer.
    unsafe {
        pg_sys::RegisterSubXactCallback(Some(tview_subxact_callback), std::ptr::null_mut());
    }

    // Initialize SAVEPOINT_DEPTH from current transaction nest level
    // When loaded inside a DO block, subtransactions may already be open
    let nest_level = unsafe { pg_sys::GetCurrentTransactionNestLevel() };
    SAVEPOINT_DEPTH.with(|d| {
        *d.borrow_mut() = (nest_level as usize).saturating_sub(1);
    });

    // Push placeholder queue snapshots for existing subtransactions
    QUEUE_SNAPSHOTS.with(|s| {
        let mut snapshots = s.borrow_mut();
        for _ in 0..(nest_level as usize).saturating_sub(1) {
            snapshots.push(HashSet::new());
        }
    });

    // Mirror the placeholders for the patch-map snapshot stack (issue #56).
    PATCH_SNAPSHOTS.with(|s| {
        let mut snapshots = s.borrow_mut();
        for _ in 0..(nest_level as usize).saturating_sub(1) {
            snapshots.push(std::collections::HashMap::new());
        }
    });
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
            #[allow(clippy::collapsible_if)]
            // Auto-enqueue suspended changes if any
            if crate::suspend::is_suspended() {
                if let Err(e) = crate::suspend::enqueue_suspended_changes() {
                    warning!("Failed to enqueue suspended changes: {}", e);
                }
            }

            // Auto-resume suspension
            crate::suspend::force_resume();

            // Queue flush + audit flush happen in ProcessUtility hook before COMMIT.
            // Clear audit buffer as safety net (should already be empty after flush).
            crate::audit::clear_audit_buffer();
            super::ops::clear_crash_recovery_cache();
            crate::metrics::metrics_api::reset_metrics();
        }
        XactEvent::Prepare => {
            // PREPARE TRANSACTION also goes through ProcessUtility hook.
            crate::audit::clear_audit_buffer();
            crate::metrics::metrics_api::reset_metrics();
        }
        XactEvent::Abort => {
            // Auto-resume suspension on abort (discard changes)
            crate::suspend::force_resume();

            clear_queue();
            super::patch::clear_patch_map();
            super::ops::clear_crash_recovery_cache();
            super::cache::cascade_cache::clear_cache();
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

                // Snapshot the patch map in lockstep (issue #56).
                let patch_snapshot = super::patch::take_patch_snapshot();
                PATCH_SNAPSHOTS.with(|s| {
                    s.borrow_mut().push(patch_snapshot);
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

                // Restore the patch map in lockstep (issue #56).
                if let Some(patch_snapshot) = PATCH_SNAPSHOTS.with(|s| s.borrow_mut().pop()) {
                    super::patch::replace_patch_map(patch_snapshot);
                }
            }
            pg_sys::SubXactEvent::SUBXACT_EVENT_COMMIT_SUB => {
                // RELEASE SAVEPOINT: just decrement depth and discard snapshot
                decrement_savepoint_depth();

                // Discard the snapshots (savepoint committed)
                QUEUE_SNAPSHOTS.with(|s| {
                    s.borrow_mut().pop();
                });
                PATCH_SNAPSHOTS.with(|s| {
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

    // Issue #56: drain the direct-patch map in lockstep with the queue so it never
    // outlives its queue entries. Keys carrying a usable `Direct` chain are patched
    // straight into tv_<entity>; everything else recomputes.
    let mut patches = super::patch::take_patch_snapshot();

    // Start timing the entire refresh operation
    let refresh_timer = crate::metrics::metrics_api::record_refresh_start();

    // Load dependency graph once (cached)
    let graph = super::cache::graph_cache::load_cached()?;

    // Track processed keys to avoid duplicates
    // Pre-allocate with capacity based on initial pending size
    let mut processed: std::collections::HashSet<super::key::RefreshKey> =
        std::collections::HashSet::with_capacity(pending.len().max(16));

    // Parent metadata cache for patch derivation (issue #56) — avoids
    // reloading a parent entity's TviewMeta once per discovered parent key.
    let mut parent_meta_cache: std::collections::HashMap<
        String,
        Option<crate::catalog::TviewMeta>,
    > = std::collections::HashMap::new();

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
                // Check for post-crash truncation and auto-refresh if needed
                if !is_crash_recovery_checked(&entity) {
                    mark_crash_recovery_checked(&entity);
                    if crate::lifecycle::detect_post_crash_truncation(&entity)? {
                        // TVIEW is empty but backing view has data - perform full refresh first
                        Spi::run_with_args(
                            "SELECT pg_tviews_refresh($1)",
                            &[unsafe {
                                DatumWithOid::new(
                                    &entity,
                                    PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
                                )
                            }],
                        )?;
                    }
                }

                // Issue #56: split off keys carrying a usable direct patch. They are
                // applied straight to tv_<entity> (no backing-view query); everything
                // else — poisoned keys, dedup keys, keys with no patch, or the fast
                // path disabled — recomputes exactly as before. The GUC is re-checked
                // here so toggling it off between capture and commit forces recompute.
                let apply_enabled = crate::config::direct_patch_enabled();
                let mut patched: Vec<(i64, Vec<super::patch::PatchEntry>)> = Vec::new();
                let mut applied_pks: HashSet<i64> = HashSet::new();
                let mut recompute_keys: Vec<super::key::RefreshKey> = Vec::new();
                for key in entity_keys {
                    if apply_enabled
                        && !key.is_dedup()
                        && let Some(super::patch::PatchState::Direct(chain)) = patches.get(&key)
                    {
                        patched.push((key.pk, chain.clone()));
                        applied_pks.insert(key.pk);
                        continue;
                    }
                    recompute_keys.push(key);
                }

                // Apply direct patches; any pk whose tview row is missing falls back.
                if !patched.is_empty() {
                    let meta =
                        crate::catalog::TviewMeta::load_by_entity(&entity)?.ok_or_else(|| {
                            crate::TViewError::MetadataNotFound {
                                entity: entity.clone(),
                            }
                        })?;
                    let fallback = crate::refresh::direct::apply_entity_patches(&meta, patched)?;
                    for pk in fallback {
                        applied_pks.remove(&pk);
                        recompute_keys.push(super::key::RefreshKey::pk(&entity, pk));
                    }
                }

                // Recompute the remaining keys via the existing single/bulk path.
                // A recomputed child's whole document changed, so its parents must
                // recompute too: poison any parent patch (issue #56).
                if recompute_keys.len() == 1 {
                    let key = &recompute_keys[0];
                    let parents = refresh_and_get_parents(key, &graph)?;
                    for parent_key in parents {
                        super::patch::poison_into(&mut patches, parent_key.clone());
                        if !processed.contains(&parent_key) {
                            pending.insert(parent_key);
                        }
                    }
                } else if recompute_keys.len() > 1 {
                    let mut pks =
                        Vec::with_capacity(recompute_keys.iter().filter(|k| !k.is_dedup()).count());
                    for key in &recompute_keys {
                        if !key.is_dedup() {
                            pks.push(key.pk);
                        }
                    }
                    // FAIL-FAST: Propagate error immediately to abort transaction
                    crate::refresh::refresh_bulk(&entity, &pks)?;

                    let parent_map = crate::propagate::find_parents_batch(&recompute_keys, &graph)?;
                    for parent_keys in parent_map.values() {
                        for parent_key in parent_keys {
                            super::patch::poison_into(&mut patches, parent_key.clone());
                            if !processed.contains(parent_key) {
                                pending.insert(parent_key.clone());
                            }
                        }
                    }
                }

                // Parent patch derivation for the patched keys that applied (issue
                // #56). For each parent embedding the child via a
                // nested_object dependency, prepend the dependency path to the
                // child's chain and record it for the parent; where a patch can't be
                // derived (array/scalar/uuid-fk dep, or the parent itself gated), the
                // parent is poisoned and recomputes. Poison stickiness means a parent
                // reached by both a patched and a recomputed child recomputes.
                if !applied_pks.is_empty() {
                    let applied_keys: Vec<super::key::RefreshKey> = applied_pks
                        .iter()
                        .map(|&pk| super::key::RefreshKey::pk(&entity, pk))
                        .collect();
                    let parent_map = crate::propagate::find_parents_batch(&applied_keys, &graph)?;
                    for (child_key, parent_keys) in &parent_map {
                        // Snapshot the child's applied chain before mutating `patches`.
                        let child_chain = match patches.get(child_key) {
                            Some(super::patch::PatchState::Direct(chain)) => Some(chain.clone()),
                            _ => None,
                        };
                        for parent_key in parent_keys {
                            let derived = match &child_chain {
                                Some(chain) => {
                                    load_meta_cached(&parent_key.entity, &mut parent_meta_cache)?
                                        .and_then(|m| {
                                            crate::refresh::direct::derive_parent_chain(
                                                &m,
                                                &child_key.entity,
                                                chain,
                                            )
                                        })
                                }
                                None => None,
                            };
                            match derived {
                                Some(chain) => super::patch::merge_chain_into(
                                    &mut patches,
                                    parent_key.clone(),
                                    chain,
                                ),
                                None => {
                                    super::patch::poison_into(&mut patches, parent_key.clone());
                                }
                            }
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
        // Merge patches captured by triggers that fired during refresh (issue #56).
        for (k, v) in super::patch::take_patch_snapshot() {
            patches.insert(k, v);
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

/// Load and cache a parent entity's `TviewMeta` for patch derivation (issue #56).
///
/// The cache lives for the duration of one flush; parent entities are few, so this
/// keeps derivation from re-querying `pg_tview_meta` per discovered parent key.
fn load_meta_cached(
    entity: &str,
    cache: &mut std::collections::HashMap<String, Option<crate::catalog::TviewMeta>>,
) -> spi::Result<Option<crate::catalog::TviewMeta>> {
    if let Some(meta) = cache.get(entity) {
        return Ok(meta.clone());
    }
    let meta = crate::catalog::TviewMeta::load_by_entity(entity)?;
    cache.insert(entity.to_string(), meta.clone());
    Ok(meta)
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

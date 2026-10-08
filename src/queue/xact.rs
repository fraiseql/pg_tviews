use super::ops::{is_crash_recovery_checked, mark_crash_recovery_checked};
use super::state;
use crate::TViewResult;
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::collections::HashSet;
use std::os::raw::c_void;

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

    // Loaded inside a DO block: subtransactions may already be open. Mark them,
    // so their end events pair with a savepoint.
    let nest_level = unsafe { pg_sys::GetCurrentTransactionNestLevel() };
    for _ in 0..usize::try_from(nest_level).unwrap_or(0).saturating_sub(1) {
        super::savepoint::start();
    }
}

/// Transaction callback handler (invoked by `PostgreSQL`)
///
/// Runs at the end of every transaction, however it ends. It only drops
/// in-memory state and reports: no SPI is allowed here, and nothing in it may
/// fail (`#[pg_guard]` turns a bug's panic into an ERROR instead of unwinding
/// through `xact.c`).
#[pg_guard]
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
            // Suspended without resuming: the hook caught up before an explicit
            // COMMIT; an implicit commit ends here, where no SPI is allowed.
            if crate::suspend::is_suspended() {
                let stale = crate::suspend::get_changed_entities();
                if !stale.is_empty() {
                    warning!(
                        "pg_tviews: transaction committed with refresh suspended; TVIEWs {:?} \
                         are stale until pg_tviews_refresh() is run for each",
                        stale
                    );
                }
                crate::suspend::clear_changed_entities();
            }

            crate::suspend::force_resume();

            // Every path that enqueues flushes before the commit (statement
            // trigger, ProcessUtility hook). Work still queued here is dropped: it
            // must never run under another transaction's snapshot, and a patch
            // carries values read in this one.
            warn_unflushed();
            // The crash-recovery check stays done for this backend: an UNLOGGED
            // TVIEW is only reset by a restart, which ends every backend.
            clear_transaction_state();
        }
        XactEvent::Prepare => {
            // The ProcessUtility hook flushed the queue before PREPARE TRANSACTION, so
            // the refresh writes are part of the prepared transaction. This backend's
            // transaction ends here: drop its in-memory state (no SPI in callbacks).
            crate::suspend::force_resume();
            super::ops::clear_crash_recovery_cache();
            clear_transaction_state();
        }
        XactEvent::Abort => {
            // The catalog the caches memoized may be rolled back with the
            // transaction: a TVIEW create that failed leaves no view, but its columns
            // were cached under the view's name (#188). Memory only, no SPI.
            super::cache::invalidate_all_caches();
            // Auto-resume suspension on abort (discard changes)
            crate::suspend::force_resume();
            crate::revision::reset();
            crate::hooks::release_hook_guard_on_abort(true);
            super::ops::clear_crash_recovery_cache();
            clear_transaction_state();
        }
    }
}

/// Drop everything a transaction kept in memory: the refresh queue, the direct
/// and fan-out patches, the cascade cache, the audit buffer, the metrics and the
/// affected-rows report. Run when the transaction ends, however it ends.
fn clear_transaction_state() {
    state::clear();
    super::savepoint::clear();
    crate::executor::reset();
    FLUSHING.set(false);
    super::cache::cascade_cache::clear_cache();
    crate::audit::clear_audit_buffer();
    crate::metrics::metrics_api::reset_metrics();
    super::affected::clear();
}

/// The WARNING for refresh work still queued when a transaction commits, once
/// per backend (no SPI: this runs in the transaction callback).
fn warn_unflushed() {
    let queued = super::state::get_queue_contents();
    if queued.is_empty() || !crate::utils::first_time("commit with queued refreshes") {
        return;
    }
    let entities: std::collections::BTreeSet<&str> =
        queued.iter().map(|k| k.entity.as_str()).collect();
    pg_sys::panic::ErrorReport::new(
        PgSqlErrorCode::ERRCODE_WARNING,
        format!(
            "pg_tviews: transaction committed with {} queued refreshes for {:?}; they were \
             not applied (missing flush trigger?)",
            queued.len(),
            entities
        ),
        function_name!(),
    )
    .set_hint(
        "Run tviews.pg_tviews_health_check() to find missing triggers, and \
         tviews.pg_tviews_refresh(entity) to rebuild the TVIEWs named.",
    )
    .report(PgLogLevel::WARNING);
}

/// Subtransaction callback handler (invoked by `PostgreSQL` for savepoints)
///
/// Keeps [`super::savepoint`] in step with the open subtransactions. The
/// savepoint is popped and restored first; cache work comes after, so nothing
/// that follows can leave the stack out of step. Infallible by construction;
/// `#[pg_guard]` turns a bug's panic into an ERROR.
#[pg_guard]
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn tview_subxact_callback(
    event: u32,
    _subxid: pg_sys::SubTransactionId,
    _parent_subid: pg_sys::SubTransactionId,
    _arg: *mut c_void,
) {
    match event {
        pg_sys::SubXactEvent::SUBXACT_EVENT_START_SUB => super::savepoint::start(),
        pg_sys::SubXactEvent::SUBXACT_EVENT_COMMIT_SUB => super::savepoint::commit(),
        pg_sys::SubXactEvent::SUBXACT_EVENT_ABORT_SUB => {
            super::savepoint::abort();
            // A CTAS that failed inside this subtransaction never reached the event
            // trigger; its pending SELECT must not leak into a later statement.
            crate::hooks::release_hook_guard_on_abort(false);
            // DDL rolled back with the subtransaction: forget what was cached of it.
            super::cache::invalidate_all_caches();
        }
        _ => {}
    }
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
/// 2. Process one entity at a time in dependency order (children before parents)
/// 3. Discover parent refreshes during processing
/// 4. Add parents to local pending queue
/// 5. Repeat until no more refreshes discovered (fixpoint)
///
/// # Correctness
///
/// - Each (entity, pk) processed exactly once (tracked in `processed` set)
/// - Dependency order respected (the next entity is the first pending one in
///   topological order; propagation only adds entities that come after it)
/// - Propagation coalesced (parents discovered during refresh added to queue)
/// - Transaction-safe (fail-fast aborts transaction on first error)
pub fn flush_refresh_queue() -> TViewResult<()> {
    // A refresh write can fire a flush trigger (a TVIEW's table read by another
    // TVIEW): that call returns at once, and the running flush drains what it
    // queued before finishing.
    let Some(_flushing) = Flushing::enter() else {
        return Ok(());
    };
    flush_pending()
}

thread_local! {
    static FLUSHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While alive, a flush is running in this backend. Dropped when the flush
/// returns or unwinds; the transaction's end clears it too.
struct Flushing;

impl Flushing {
    fn enter() -> Option<Self> {
        if FLUSHING.replace(true) {
            return None;
        }
        Some(Self)
    }
}

impl Drop for Flushing {
    fn drop(&mut self) {
        FLUSHING.set(false);
    }
}

fn flush_pending() -> TViewResult<()> {
    // Take what the triggers queued. The direct patches come with their queue
    // entries (issue #56): keys carrying a usable `Direct` chain are patched
    // straight into tv_<entity>; everything else recomputes.
    let state::Pending {
        queue: mut pending,
        mut patches,
        fanout: fanouts,
    } = state::drain();

    if pending.is_empty() && fanouts.is_empty() {
        return Ok(());
    }
    crate::revision::check();
    crate::config::warn_deprecated_settings();
    super::affected::begin_flush();

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

    // Issue #120: write each parent change into all its children at once; the
    // parents of every changed child join the queue.
    apply_fanouts(fanouts, &graph, &mut patches, &mut pending, &processed)?;

    // Outer drain loop: after the inner loop empties `pending`, check for
    // late-enqueued items from triggers that fired during refresh (e.g.,
    // pg_treekey cascading child rows in tb_location).  The `processed` set
    // carries across drain passes so already-refreshed keys are not repeated.
    let mut iteration = 1;
    loop {
        // Inner loop: process pending until empty (propagation via parents)
        while !pending.is_empty() {
            // Refresh one entity per pass, the first in dependency order. The
            // parents its refresh discovers come later in that order, so every key
            // is refreshed after everything it reads (a view over another tv_*
            // table sees fresh rows), and only once.
            let sorted_keys = graph.sort_keys(pending.drain().collect());
            let entity = sorted_keys[0].entity.clone();
            let mut entity_keys = Vec::new();
            for key in sorted_keys {
                if key.entity != entity {
                    pending.insert(key);
                } else if processed.insert(key.clone()) {
                    entity_keys.push(key);
                }
            }

            // The entity is read and written as the owner of its tv_* table
            // (issue #136), whoever wrote to the base table.
            let _owner = crate::owner::AsOwner::of_entity(&entity)?;

            // Check for post-crash truncation and auto-refresh if needed
            if !is_crash_recovery_checked(&entity) {
                mark_crash_recovery_checked(&entity);
                if crate::lifecycle::detect_post_crash_truncation(&entity)? {
                    // The TVIEW is empty but its view is not: fill it. No TRUNCATE, so
                    // no ACCESS EXCLUSIVE lock held until the transaction ends.
                    crate::admin::fill_empty_tview(&entity)?;
                }
            }

            // A write to a table no cascade maps (`full_refresh` policy, issues
            // #157, #158): bring the whole TVIEW to its view once, which covers
            // every other key of the entity, and queue the parents of the rows
            // that changed.
            if entity_keys.iter().any(super::key::RefreshKey::is_all) {
                let meta =
                    crate::catalog::TviewMeta::load_by_entity(&entity)?.ok_or_else(|| {
                        crate::TViewError::MetadataNotFound {
                            entity: entity.clone(),
                        }
                    })?;
                let changed: Vec<i64> = crate::ddl::replace::reconcile(&entity, &meta)?
                    .iter()
                    .filter_map(|k| k.parse::<i64>().ok())
                    .collect();
                // A full refresh may bring rows back: look their parents up in the
                // parents' views too.
                for parent_key in
                    crate::propagate::find_parents_batch(&entity, &changed, &changed, &graph)?
                        .into_values()
                        .flatten()
                {
                    super::patch::poison_into(&mut patches, parent_key.clone());
                    if !processed.contains(&parent_key) {
                        pending.insert(parent_key);
                    }
                }
                if meta.identity.is_pk(&entity) {
                    processed.extend(
                        changed
                            .into_iter()
                            .map(|pk| super::key::RefreshKey::pk(&entity, pk)),
                    );
                }
                iteration += 1;
                continue;
            }

            // Issue #56: split off keys carrying a usable direct patch. They are
            // applied straight to tv_<entity> (no backing-view query); everything
            // else — poisoned keys, keys with no patch, or the fast path
            // disabled — recomputes exactly as before. The GUC is re-checked
            // here so toggling it off between capture and commit forces recompute.
            let apply_enabled = crate::config::direct_patch_enabled();
            let mut patched: Vec<(i64, Vec<super::patch::PatchEntry>)> = Vec::new();
            let mut applied_pks: HashSet<i64> = HashSet::new();
            let mut recompute_keys: Vec<super::key::RefreshKey> = Vec::new();
            for key in entity_keys {
                if apply_enabled
                    && let Some(pk) = key.key.as_int()
                    && let Some(super::patch::PatchState::Direct(chain)) = patches.get(&key)
                {
                    patched.push((pk, chain.clone()));
                    applied_pks.insert(pk);
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

            // Recompute the remaining keys, one row with a smart patch or several
            // in bulk; both return the pk_<entity> of the rows they touched, which
            // parents look them up by (ADR 0169). A recomputed child's whole
            // document changed, so its parents must recompute too: poison any
            // parent patch (issue #56). FAIL-FAST: an error aborts the transaction.
            if !recompute_keys.is_empty() {
                let keys: Vec<super::key::KeyValue> =
                    recompute_keys.into_iter().map(|k| k.key).collect();
                let touched = if let [key] = keys.as_slice() {
                    let meta =
                        crate::catalog::TviewMeta::load_by_entity(&entity)?.ok_or_else(|| {
                            crate::TViewError::MetadataNotFound {
                                entity: entity.clone(),
                            }
                        })?;
                    crate::refresh::refresh_key(&meta, key)?
                } else {
                    crate::refresh::refresh_bulk(&entity, &keys)?
                };
                for parent_key in crate::propagate::find_parents_batch(
                    &entity,
                    &touched.pks,
                    &touched.appeared,
                    &graph,
                )?
                .into_values()
                .flatten()
                {
                    super::patch::poison_into(&mut patches, parent_key.clone());
                    if !processed.contains(&parent_key) {
                        pending.insert(parent_key);
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
                let applied: Vec<i64> = applied_pks.into_iter().collect();
                let parent_map =
                    crate::propagate::find_parents_batch(&entity, &applied, &[], &graph)?;
                for (child_pk, parent_keys) in &parent_map {
                    let child_key = &super::key::RefreshKey::pk(&entity, *child_pk);
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

        // Drain any items enqueued by triggers that fired during refresh, with
        // the patches they captured (issue #56).
        let late = state::drain();
        if late.queue.is_empty() && late.fanout.is_empty() {
            break;
        }
        patches.extend(late.patches);
        let late_fanouts = late.fanout;
        pending = late.queue;
        apply_fanouts(late_fanouts, &graph, &mut patches, &mut pending, &processed)?;
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

/// Apply fan-out patches (issue #120): one UPDATE per child entity and lookup
/// column, writing each parent's changed fields into all its children. Every
/// child row that changed is journaled, and its own parents are queued (to
/// recompute: they embed the child's document).
fn apply_fanouts(
    fanouts: super::patch::FanoutMap,
    graph: &super::graph::EntityDepGraph,
    patches: &mut std::collections::HashMap<super::key::RefreshKey, super::patch::PatchState>,
    pending: &mut HashSet<super::key::RefreshKey>,
    processed: &HashSet<super::key::RefreshKey>,
) -> TViewResult<()> {
    let mut groups: std::collections::BTreeMap<(String, String), Vec<_>> =
        std::collections::BTreeMap::new();
    for ((entity, lookup_col, key), fields) in fanouts {
        groups
            .entry((entity, lookup_col))
            .or_default()
            .push((key, fields));
    }
    for ((entity, lookup_col), rows) in groups {
        let meta = crate::catalog::TviewMeta::load_by_entity(&entity)?.ok_or_else(|| {
            crate::TViewError::MetadataNotFound {
                entity: entity.clone(),
            }
        })?;
        let owner = crate::owner::AsOwner::of_table(meta.tview_oid)?;
        let changed = crate::refresh::direct::apply_fanout_patch(&meta, &lookup_col, &rows)?;
        drop(owner);
        for parent_key in crate::propagate::find_parents_batch(&entity, &changed, &[], graph)?
            .into_values()
            .flatten()
        {
            super::patch::poison_into(patches, parent_key.clone());
            if !processed.contains(&parent_key) {
                pending.insert(parent_key);
            }
        }
    }
    Ok(())
}

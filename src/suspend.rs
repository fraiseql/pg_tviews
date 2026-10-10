//! Suspension of trigger-based refresh for bulk work.
//!
//! `pg_tviews_suspend_triggers()` makes the triggers record which TVIEWs a
//! write touched instead of refreshing them; resuming rebuilds those TVIEWs.
//! Suspension lasts until the transaction ends, and follows subtransactions: a
//! savepoint records it, and rolling the savepoint back restores it.

use std::cell::RefCell;
use std::collections::BTreeSet;

/// Whether refresh is suspended, and what changed meanwhile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Suspension {
    /// Nesting of suspend calls not yet resumed; suspended while above 0.
    depth: u32,
    /// The entities written while suspended.
    changed: BTreeSet<String>,
}

thread_local! {
    static SUSPENSION: RefCell<Suspension> = RefCell::default();
}

/// Suspend trigger-based refresh.
pub fn suspend() {
    SUSPENSION.with_borrow_mut(|s| {
        s.depth += 1;
        if s.depth == 1 {
            s.changed.clear();
        }
    });
}

/// Resume trigger-based refresh (one level of nesting).
pub fn resume() -> crate::TViewResult<()> {
    SUSPENSION.with_borrow_mut(|s| {
        if s.depth == 0 {
            return Err(crate::TViewError::WrongState {
                reason: "Cannot resume: not suspended".to_string(),
            });
        }
        s.depth -= 1;
        Ok(())
    })
}

/// Whether trigger-based refresh is currently suspended.
#[must_use]
pub fn is_suspended() -> bool {
    SUSPENSION.with_borrow(|s| s.depth > 0)
}

/// Record that an entity changed while triggers are suspended.
pub fn record_change(entity_name: &str) {
    SUSPENSION.with_borrow_mut(|s| {
        if s.depth > 0 {
            s.changed.insert(entity_name.to_string());
        }
    });
}

/// The entities that changed while suspended.
#[must_use]
pub fn get_changed_entities() -> Vec<String> {
    SUSPENSION.with_borrow(|s| s.changed.iter().cloned().collect())
}

/// Forget the entities that changed while suspended.
pub fn clear_changed_entities() {
    SUSPENSION.with_borrow_mut(|s| s.changed.clear());
}

/// The suspension now, for a savepoint to restore.
pub fn snapshot() -> Suspension {
    SUSPENSION.with_borrow(Clone::clone)
}

/// A savepoint rolled back: the suspension is what it was when it started.
pub fn restore(saved: Suspension) {
    SUSPENSION.with_borrow_mut(|s| *s = saved);
}

/// Rebuild every TVIEW changed while refresh was suspended, and every TVIEW that
/// embeds one of them, dependencies first, then forget the recorded changes.
/// Returns the rebuilt entities in order. Needs SPI (a function call or the
/// `ProcessUtility` hook, never a transaction callback).
///
/// # Errors
/// Returns an error if loading the dependency graph or a rebuild fails.
pub fn catch_up() -> crate::TViewResult<Vec<String>> {
    let changed = get_changed_entities();
    clear_changed_entities();
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    // A TVIEW whose view reads a rebuilt one is stale too.
    crate::admin::rebuild_with_dependents(&changed)
}

/// Resume however deeply suspended: the transaction ended.
pub fn force_resume() {
    SUSPENSION.with_borrow_mut(|s| s.depth = 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restored_snapshot_undoes_suspend_and_resume() {
        force_resume();
        clear_changed_entities();
        let before = snapshot();
        suspend();
        record_change("post");
        assert!(is_suspended());
        restore(before);
        assert!(!is_suspended());
        assert!(get_changed_entities().is_empty());

        suspend();
        let suspended = snapshot();
        resume().unwrap();
        assert!(!is_suspended());
        restore(suspended);
        assert!(is_suspended());
        force_resume();
    }

    #[test]
    fn resume_without_suspend_is_refused() {
        force_resume();
        assert!(resume().is_err());
    }
}

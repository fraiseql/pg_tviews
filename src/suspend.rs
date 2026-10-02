use std::collections::HashSet;
use std::sync::Mutex;

/// Global runtime state for trigger suspension
pub struct SuspensionState {
    /// Is suspension active in this session?
    pub is_suspended: bool,
    /// Which entities changed while suspended (entity names)
    pub changed_entities: HashSet<String>,
    /// Nesting depth for nested suspend/resume calls
    pub suspension_depth: i32,
}

lazy_static::lazy_static! {
    static ref SUSPENSION_STATE: Mutex<SuspensionState> = Mutex::new(SuspensionState {
        is_suspended: false,
        changed_entities: HashSet::new(),
        suspension_depth: 0,
    });
}

/// Suspend trigger-based refresh
pub fn suspend() -> Result<(), String> {
    let mut state = SUSPENSION_STATE.lock().unwrap();
    state.suspension_depth += 1;
    if state.suspension_depth == 1 {
        state.is_suspended = true;
        state.changed_entities.clear();
    }
    Ok(())
}

/// Resume trigger-based refresh
pub fn resume() -> Result<(), String> {
    let mut state = SUSPENSION_STATE.lock().unwrap();
    if state.suspension_depth == 0 {
        return Err("Cannot resume: not suspended".to_string());
    }
    state.suspension_depth -= 1;
    if state.suspension_depth == 0 {
        state.is_suspended = false;
    }
    Ok(())
}

/// Check if trigger-based refresh is currently suspended
#[must_use]
pub fn is_suspended() -> bool {
    SUSPENSION_STATE.lock().unwrap().is_suspended
}

/// Record that an entity changed while triggers are suspended
pub fn record_change(entity_name: &str) {
    if is_suspended() {
        SUSPENSION_STATE
            .lock()
            .unwrap()
            .changed_entities
            .insert(entity_name.to_string());
    }
}

/// Get list of entities that changed while suspended
#[must_use]
pub fn get_changed_entities() -> Vec<String> {
    SUSPENSION_STATE
        .lock()
        .unwrap()
        .changed_entities
        .iter()
        .cloned()
        .collect()
}

/// Clear the list of changed entities
pub fn clear_changed_entities() {
    SUSPENSION_STATE.lock().unwrap().changed_entities.clear();
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
    crate::admin::rebuild_with_dependents(&changed, false)
}

/// Force resume (used by transaction callback)
pub fn force_resume() {
    let mut state = SUSPENSION_STATE.lock().unwrap();
    state.suspension_depth = 0;
    state.is_suspended = false;
}

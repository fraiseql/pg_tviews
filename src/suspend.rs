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

/// Enqueue suspended changes for refresh
pub fn enqueue_suspended_changes() -> Result<(), String> {
    let entities = get_changed_entities();
    for entity in entities {
        crate::queue::enqueue_refresh(&entity, 0);
    }
    clear_changed_entities();
    Ok(())
}

/// Force resume (used by transaction callback)
pub fn force_resume() {
    let mut state = SUSPENSION_STATE.lock().unwrap();
    state.suspension_depth = 0;
    state.is_suspended = false;
}

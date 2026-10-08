//! The base tables a backing view reads (through other views, from `pg_depend`),
//! and the triggers `pg_tviews` installs on them for each TVIEW, as its plan says
//! (`trigger_plan`).

pub mod base_tables;
pub mod triggers;

pub use base_tables::find_base_tables;
pub use triggers::{
    TriggerPlan, install_triggers, remove_entity_triggers, sync_entity_triggers, trigger_plan,
};

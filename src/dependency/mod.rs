//! The base tables a backing view reads (through other views, from `pg_depend`),
//! and the one set of triggers `pg_tviews` keeps on each table its TVIEWs read,
//! as their plans say (`sync_tables`).

pub mod base_tables;
pub mod triggers;

pub use base_tables::{base_tables_of, find_base_tables};
pub use triggers::{
    TriggerPlan, check_installable, sync_after_drop, sync_read_tables, sync_tables, trigger_plan,
};

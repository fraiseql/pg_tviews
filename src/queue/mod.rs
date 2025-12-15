//! Transaction-level refresh queue for coalesced TVIEW updates
//!
//! This module implements the transaction queue architecture from `PRD_multiupdate.md`:
//! - `RefreshKey`: Identifies unique (entity, pk) pairs
//! - `TX_REFRESH_QUEUE`: Thread-local `HashSet` for deduplication
//! - Enqueue/dequeue operations
//! - Transaction callback registration

pub mod key;
mod state;
mod ops;
mod xact;
mod graph;
pub mod cache;
pub mod persistence;
mod integration_tests;

pub use key::RefreshKey;
pub use ops::{enqueue_refresh, enqueue_refresh_bulk, register_commit_callback_once, get_queue_stats};
pub use state::{get_queue_size, get_queue_contents};
// pub use ops::{take_queue_snapshot, clear_queue}; // Internal use only
// pub use graph::EntityDepGraph; // Internal use only
// Internal use only (not exported):
// - TX_REFRESH_QUEUE
// - TX_REFRESH_SCHEDULED
// - reset_scheduled_flag
// - xact module
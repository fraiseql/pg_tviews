//! Transaction-level refresh queue for coalesced TVIEW updates
//!
//! This module implements the transaction queue architecture from PRD_multiupdate.md:
//! - RefreshKey: Identifies unique (entity, pk) pairs
//! - TX_REFRESH_QUEUE: Thread-local HashSet for deduplication
//! - Enqueue/dequeue operations
//! - Transaction callback registration

mod key;
mod state;
mod ops;
mod xact;
mod graph;
mod integration_tests;

pub use key::RefreshKey;
pub use ops::{enqueue_refresh, register_commit_callback_once};
// pub use ops::{take_queue_snapshot, clear_queue}; // Internal use only
// pub use graph::EntityDepGraph; // Internal use only
// Internal use only (not exported):
// - TX_REFRESH_QUEUE
// - TX_REFRESH_SCHEDULED
// - reset_scheduled_flag
// - xact module
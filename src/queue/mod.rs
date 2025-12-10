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
mod integration_tests;

pub use key::RefreshKey;
#[allow(unused_imports)]
pub use ops::{enqueue_refresh, take_queue_snapshot, clear_queue, register_commit_callback_once};
// Internal use only (not exported):
// - TX_REFRESH_QUEUE
// - TX_REFRESH_SCHEDULED
// - reset_scheduled_flag
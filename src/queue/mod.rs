//! Transaction-level refresh queue for coalesced TVIEW updates
//!
//! This module implements the transaction queue architecture from `PRD_multiupdate.md`:
//! - `RefreshKey`: Identifies unique (entity, pk) pairs
//! - `TX_REFRESH_QUEUE`: Thread-local `HashSet` for deduplication
//! - Enqueue/dequeue operations
//! - Transaction callback registration

pub mod cache;
pub mod graph;
mod integration_tests;
pub mod key;
mod ops;
pub mod patch;
mod state;
pub mod xact;

pub use graph::EntityDepGraph;
pub use key::RefreshKey;
pub use ops::{
    enqueue_refresh, enqueue_refresh_bulk, enqueue_refresh_dedup, enqueue_refresh_patched,
    is_queue_empty, spi_batch_lookup,
};
pub use state::{get_queue_contents, get_queue_size};
pub use xact::flush_refresh_queue;

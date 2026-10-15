//! The transaction's pending refresh work: the keys the triggers queued, the
//! patches they captured, the rows the flush touched, and the undo log a
//! rolled-back subtransaction restores. [`crate::flush`] applies it.

pub mod affected;
pub mod key;
pub(crate) mod ops;
pub mod patch;
pub(crate) mod state;

pub use key::RefreshKey;
pub use ops::{
    enqueue_refresh, enqueue_refresh_all, enqueue_refresh_bulk, enqueue_refresh_patched,
};

pub use state::{get_queue_contents, get_queue_size};

/// The kind of write a statement made, as its statement trigger fired for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Event {
    Insert,
    Update,
    Delete,
}

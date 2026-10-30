//! Map a statement's changed rows to TVIEW keys (ADR 0157).
//!
//! A `mapped` base table carries, per TVIEW, three statement-level triggers that
//! see the statement's transition tables (`INSERT`, `UPDATE`, `DELETE`: a trigger
//! with transition tables takes one event). The handler runs the table's mapping
//! query once over the changed rows, as the TVIEW's owner, and enqueues the keys it
//! returns. `all_keys` tables carry the same triggers: under the `full_refresh`
//! policy they enqueue the whole TVIEW. A partitioned table maps each row from the
//! row trigger instead: `PostgreSQL` copies only row triggers onto partitions, and
//! the transition tables of the root's statement triggers would miss the rows of
//! statements naming a partition. `TRUNCATE` refreshes the whole TVIEW, once per
//! statement however many truncated partitions fire.

mod mapping;
mod partition;
mod render;
mod transition;
mod trigger;

pub use crate::queue::Event;
pub(crate) use mapping::{mapping_query, read_set_queries};
pub use partition::{map_entering_partition, map_leaving_partition, map_row, refresh_tviews_over};
pub use trigger::{begin_truncate, require_level, trigger_entity};

//! Lineage of a TVIEW: how a write to each base table its backing view reads
//! maps to TVIEW keys (ADR 0157).
//!
//! [`walk`] reads PostgreSQL's analyzed query of the backing view (views, CTEs and
//! subqueries expanded) into a small graph: every occurrence of a base table, the
//! TVIEW key, and the predicates that link two occurrences. It is the only code
//! that touches `pg_sys` nodes. Everything else here is plain Rust over that graph:
//! each table is classified as
//!
//! - `Local(col)`: the key is a column of the changed row (the root table, a table
//!   joined on `col = <key>`); the row trigger reads it;
//! - `Mapped`: a chain of predicates links the table to the key; a query over the
//!   changed rows returns the keys;
//! - `Propagated(entity)`: read through a TVIEW this one embeds, joined on that
//!   TVIEW's key: refreshing it refreshes the rows holding its key (the plan's
//!   embed lookups);
//! - `AllKeys`: nothing selective links it to the key; the TVIEW's `uncascaded_policy`
//!   decides.
//!
//! A predicate is used only as a *necessary* condition for a changed row to
//! contribute to a TVIEW row, so leaving one out only widens the mapping. Only
//! strict, immutable predicates over two table occurrences are kept, and only in the
//! directions in which they must hold: both ways for `WHERE` and inner-join
//! conditions, from the nullable side for an outer join, and from a subquery to the
//! query around it.

mod analyze;
mod graph;
mod identity;
mod template;
pub mod walk;

pub use analyze::*;
pub use graph::*;
pub use identity::*;
pub use template::*;

#[cfg(test)]
mod tests;

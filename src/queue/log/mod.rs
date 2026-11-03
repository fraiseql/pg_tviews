//! The transaction's write log: every TVIEW row a refresh write changed,
//! recorded once ([`record`]) and read by two readers, each with its own
//! retention: the report of `pg_tviews_flush_and_report` ([`report`]: capped by
//! 10,000 rows, compacted as it reports) and the change feed
//! commit publishes ([`feed`]: the whole transaction, within a byte budget,
//! folded by row). One savepoint mark ([`Mark`]) covers both, so a rolled-back
//! write is neither reported nor published.

pub mod feed;
pub mod report;

pub use report::{Change, NetChange, Recorded, Summary, begin_flush, changed_in_flush, summarize};

/// Record that a refresh write changed a TVIEW row: in the flush's changed rows,
/// `tviews.stats`, the report and the feed.
pub fn record(recorded: Recorded) {
    let counter = match recorded.change {
        Change::Deleted => crate::stats::Counter::RowsDeleted,
        _ => crate::stats::Counter::RowsWritten,
    };
    crate::stats::add(&recorded.entity, counter, 1);
    report::note_flush_change(&recorded);
    if let Some(publish) = crate::config::publish_changes() {
        feed::add(publish, &recorded);
    }
    report::append(recorded);
}

/// Record that `entity`'s TVIEW was rebuilt or refilled whole, its rows not
/// recorded one by one: the report lists its type in `invalidated_types`, and
/// commit publishes the type by name.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn record_rebuilt(entity: &str) -> crate::TViewResult<()> {
    report::append_rebuilt(entity);
    if crate::config::publish_changes().is_some()
        && let Some(meta) = crate::catalog::TviewMeta::load_by_entity(entity)?
    {
        feed::add_type(&meta.typename);
    }
    Ok(())
}

/// Where the log stood when a subtransaction started, for both readers.
#[derive(Debug)]
pub struct Mark {
    report: report::Mark,
    feed: feed::Mark,
}

/// A subtransaction started: where the log stands.
pub fn mark() -> Mark {
    Mark {
        report: report::mark(),
        feed: feed::mark(),
    }
}

/// The subtransaction of `mark` committed: its writes belong to its parent.
pub fn release(mark: Mark) {
    report::release(mark.report);
}

/// The subtransaction of `mark` rolled back: forget the writes it recorded.
pub fn rollback(mark: Mark) {
    report::rollback(mark.report);
    feed::rollback(mark.feed);
}

/// Forget everything: the transaction ended.
pub fn clear() {
    report::clear();
    feed::clear();
}

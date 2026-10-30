//! The triggers' entry points: the statement-level trigger over a `mapped` or
//! `all_keys` table's transition tables, and the `TRUNCATE` trigger, refreshing
//! each TVIEW once per statement.

use super::Event;
use super::mapping::map_statement;
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::cell::RefCell;

thread_local! {
    /// Number of the current `TRUNCATE` statement, counted by the `ProcessUtility`
    /// hook; 0 when the hook is not loaded.
    static TRUNCATE_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The entities the `TRUNCATE` statement numbered `.0` already refreshed.
    static TRUNCATE_REFRESHED: RefCell<(u64, std::collections::HashSet<String>)> =
        RefCell::new((0, std::collections::HashSet::new()));
    /// The `(entity, relation)` pairs whose rows the `TRUNCATE` statement numbered
    /// `.0` mapped to keys before it ran (partitions of a tree the TVIEW reads):
    /// their truncate triggers refresh nothing in full.
    static TRUNCATE_MAPPED: RefCell<(u64, std::collections::HashSet<(String, pg_sys::Oid)>)> =
        RefCell::new((0, std::collections::HashSet::new()));
}

/// The `TRUNCATE` statement running mapped the rows of `relations` to `entity`'s
/// keys before it ran. Outside a `TRUNCATE` (a `DROP TABLE`, a `DETACH`), nothing
/// is recorded: no truncate trigger follows.
pub(super) fn truncate_mapped(entity: &str, relations: &[pg_sys::Oid]) {
    let epoch = TRUNCATE_EPOCH.get();
    if epoch == 0 {
        return;
    }
    TRUNCATE_MAPPED.with(|m| {
        let mut mapped = m.borrow_mut();
        if mapped.0 != epoch {
            *mapped = (epoch, std::collections::HashSet::new());
        }
        mapped
            .1
            .extend(relations.iter().map(|r| (entity.to_string(), *r)));
    });
}

/// Whether the running `TRUNCATE` mapped `relation`'s rows to `entity`'s keys.
fn was_truncate_mapped(entity: &str, relation: pg_sys::Oid) -> bool {
    let epoch = TRUNCATE_EPOCH.get();
    epoch != 0
        && TRUNCATE_MAPPED.with(|m| {
            let mapped = m.borrow();
            mapped.0 == epoch && mapped.1.contains(&(entity.to_string(), relation))
        })
}

/// The entity a `pg_tviews` trigger serves: its argument. A trigger without one
/// (an older release installed them; the upgrade drops them) fails the write.
pub fn trigger_entity(trigger: &PgTrigger<'_>) -> String {
    trigger
        .extra_args()
        .ok()
        .and_then(|args| args.into_iter().next())
        .unwrap_or_else(|| {
            let name = trigger.name().unwrap_or("?");
            crate::TViewError::TriggerMisused {
                reason: format!(
                    "pg_tviews: trigger {name} names no TVIEW; run \
                     SELECT * FROM tviews.pg_tviews_reregister_all() and drop it"
                ),
            }
            .raise()
        })
}

/// Raise `TriggerMisused` unless `trigger` fires per row (`row`) or per
/// statement: a handler attached by hand at the other level would see no row, or
/// no transition table, and refresh nothing.
pub fn require_level(trigger: &PgTrigger<'_>, row: bool) {
    if matches!(trigger.level(), PgTriggerLevel::Row) != row {
        let name = trigger.name().unwrap_or("?");
        let (fires, needs) = if row {
            ("STATEMENT", "ROW")
        } else {
            ("ROW", "STATEMENT")
        };
        crate::TViewError::TriggerMisused {
            reason: format!(
                "pg_tviews: trigger {name} fires FOR EACH {fires}, its function needs FOR EACH {needs}"
            ),
        }
        .raise()
    }
}

pub(super) fn suspended() -> bool {
    crate::suspend::is_suspended()
}

/// Statement-level trigger over the transition tables of a `mapped` or
/// `all_keys` table.
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires a Result return type
fn pg_tview_delta_trigger<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    require_level(trigger, false);
    let entity = trigger_entity(trigger);
    if suspended() {
        crate::concurrency::note_link_change();
        crate::suspend::record_change(&entity);
        return Ok(None);
    }
    let event = match trigger.op() {
        Ok(PgTriggerOperation::Insert) => Event::Insert,
        Ok(PgTriggerOperation::Update) => Event::Update,
        Ok(PgTriggerOperation::Delete) => Event::Delete,
        _ => return Ok(None),
    };
    let table_oid = match trigger.relation() {
        Ok(rel) => rel.oid(),
        Err(e) => crate::TViewError::TriggerMisused {
            reason: format!("pg_tviews: delta trigger without a relation: {e}"),
        }
        .raise(),
    };
    if let Err(e) = map_statement(trigger, &entity, table_oid, event) {
        e.raise_in(&format!(
            "pg_tviews: could not map the changed rows to tv_{entity} keys"
        ));
    }
    Ok(None)
}

/// A `TRUNCATE` statement starts: its truncate triggers refresh each TVIEW once.
/// Called by the `ProcessUtility` hook. No SPI.
pub fn begin_truncate() {
    TRUNCATE_EPOCH.set(TRUNCATE_EPOCH.get().wrapping_add(1).max(1));
}

/// Whether this `TRUNCATE` statement has yet to refresh `entity`, recording that it
/// does now. Always true without the hook (no statement numbers).
fn first_truncate_refresh(entity: &str) -> bool {
    let epoch = TRUNCATE_EPOCH.get();
    epoch == 0
        || TRUNCATE_REFRESHED.with(|r| {
            let mut refreshed = r.borrow_mut();
            if refreshed.0 != epoch {
                *refreshed = (epoch, std::collections::HashSet::new());
            }
            refreshed.1.insert(entity.to_string())
        })
}

/// `TRUNCATE` of a base table: refresh the whole TVIEW now (no flush trigger
/// follows a `TRUNCATE`). `TRUNCATE` of a partitioned table fires this on each
/// truncated partition too, after all of them are empty: the first one refreshes.
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires a Result return type
fn pg_tview_truncate_trigger<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let entity = trigger_entity(trigger);
    crate::concurrency::note_link_change();
    if suspended() {
        crate::suspend::record_change(&entity);
        return Ok(None);
    }
    // A truncated partition whose rows were mapped to keys before the statement:
    // those keys are refreshed after it, not the whole TVIEW.
    if let Ok(relation) = trigger.relation()
        && was_truncate_mapped(&entity, relation.oid())
    {
        return Ok(None);
    }
    if !first_truncate_refresh(&entity) {
        return Ok(None);
    }
    crate::queue::enqueue_refresh_all(&entity);
    // A TRUNCATE run inside a writing statement (by one of its triggers) leaves
    // the flush to that statement, as its other nested statements do.
    if crate::executor::inside_writing_statement() {
        return Ok(None);
    }
    if let Err(e) = crate::flush::flush_refresh_queue() {
        e.raise_in(&format!(
            "pg_tviews: could not refresh tv_{entity} after TRUNCATE"
        ));
    }
    Ok(None)
}

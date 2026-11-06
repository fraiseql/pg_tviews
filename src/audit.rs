//! The audit log (`pg_tviews.audit_enabled`): one row per TVIEW created,
//! dropped, or refreshed by a flush, written where it happens, so a rolled-back
//! transaction or savepoint takes its rows with it. Nothing is built while the
//! setting is off.

use std::collections::BTreeMap;

/// Write `entries` (`{op, entity, rows, details}` objects) as the extension owner,
/// which owns the log and records the session user. Runs SPI.
fn write(entries: Vec<serde_json::Value>) -> crate::TViewResult<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let payload = serde_json::Value::Array(entries).to_string();
    crate::owner::as_extension_owner(|| {
        crate::utils::spi::run(
            &format!(
                "SELECT {}.pg_tviews_audit_write($1::jsonb)",
                crate::utils::ext_schema()
            ),
            &[crate::utils::spi::text(&payload)],
        )
    })
}

fn entry(
    op: &str,
    entity: &str,
    rows: Option<i64>,
    details: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({"op": op, "entity": entity, "rows": rows, "details": details})
}

/// Audit a TVIEW's creation.
///
/// # Errors
/// Returns an error if the entry cannot be written.
pub fn log_create(entity: &str, definition: &str) -> crate::TViewResult<()> {
    if !crate::config::audit_enabled() {
        return Ok(());
    }
    write(vec![entry(
        "CREATE",
        entity,
        None,
        &serde_json::json!({
            "definition": definition,
            "version": env!("CARGO_PKG_VERSION"),
        }),
    )])
}

/// Audit a TVIEW's drop.
///
/// # Errors
/// Returns an error if the entry cannot be written.
pub fn log_drop(entity: &str) -> crate::TViewResult<()> {
    if !crate::config::audit_enabled() {
        return Ok(());
    }
    write(vec![entry("DROP", entity, None, &serde_json::Value::Null)])
}

/// Audit a flush: one entry per entity, with the number of its rows refreshed.
///
/// # Errors
/// Returns an error if the entries cannot be written.
pub fn log_refreshes<'a>(entities: impl Iterator<Item = &'a str>) -> crate::TViewResult<()> {
    if !crate::config::audit_enabled() {
        return Ok(());
    }
    write(refresh_entries(entities))
}

/// One `REFRESH` entry per entity of `entities`, counting its occurrences.
fn refresh_entries<'a>(entities: impl Iterator<Item = &'a str>) -> Vec<serde_json::Value> {
    let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
    for entity in entities {
        *counts.entry(entity).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|(entity, n)| entry("REFRESH", entity, Some(n), &serde_json::Value::Null))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flush_is_one_entry_per_entity_with_its_row_count() {
        let entries = refresh_entries(["user", "tag", "user"].into_iter());
        assert_eq!(
            entries,
            vec![
                entry("REFRESH", "tag", Some(1), &serde_json::Value::Null),
                entry("REFRESH", "user", Some(2), &serde_json::Value::Null),
            ]
        );
    }
}

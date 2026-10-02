//! Refresh Module: Smart JSONB Patching for Cascade Updates
//!
//! This module handles refreshing transformed views (TVIEWs) when underlying source
//! table rows change. It uses **smart JSONB patching** via the `jsonb_delta` extension
//! for 1.5-3× performance improvement on cascade updates.

pub mod main;

pub mod bulk;
pub mod direct;

// Re-export main functions for backward compatibility
pub use main::refresh_pk;
// Re-export DISTINCT ON refresh
pub use main::refresh_by_dedup_key;
// Re-export bulk functions
pub use bulk::refresh_bulk;

use crate::utils::quote_identifier;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Quoted, comma-separated column list of a refresh upsert (`INSERT INTO tv (…)`
/// and the matching `SELECT …`), so reserved-word and mixed-case columns work (#89).
pub(crate) fn column_list(col_names: &[String]) -> String {
    col_names
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `ON CONFLICT` action for a refresh upsert: `DO UPDATE SET <cols> = EXCLUDED.<cols>,
/// updated_at = NOW()` guarded by a comparison of the non-key columns (issue #72).
///
/// The guard compares record images ([`rows_differ`]), with the operator and type
/// qualified. The flush runs under the owner's
/// `search_path = pg_catalog, pg_temp` (#141), so a per-type `=` installed elsewhere
/// (ltree, citext, hstore) would not be found, and some types (json, point) have no
/// `=` at all (#156). `*=` needs neither. The `::record` casts stop the parser from
/// expanding `ROW(..) op ROW(..)` into one per-column `*=`. Equality is binary: NULL
/// equals NULL, but citext `'A'` vs `'a'` or numeric `1.0` vs `1.00` count as changes,
/// which is what a materialized copy should record.
///
/// A recomputed row that equals the stored one gets no new tuple version, no index
/// entries, no dead tuple, and keeps its `updated_at` ("last content change").
/// `key_col` is the conflict key (excluded from the SET list); target columns are
/// qualified with `qi_tv`, the quoted (schema-qualified) TVIEW table, because the
/// source relation is in scope too.
///
/// `data_expr` replaces `EXCLUDED.data` as the new `data` value (the smart-patch
/// path merges into the stored document). Every other column still tracks the
/// backing view, so no projected column is left stale (issue #98).
pub(crate) fn upsert_conflict_action(
    qi_tv: &str,
    col_names: &[String],
    key_col: &str,
    data_expr: Option<&str>,
) -> String {
    let cols: Vec<(String, String)> = col_names
        .iter()
        .filter(|c| c.as_str() != key_col)
        .map(|c| {
            let q = quote_identifier(c);
            let fresh = match data_expr {
                Some(expr) if c == "data" => expr.to_string(),
                _ => format!("EXCLUDED.{q}"),
            };
            (q, fresh)
        })
        .collect();
    if cols.is_empty() {
        return "DO NOTHING".to_string();
    }
    let set = cols
        .iter()
        .map(|(c, fresh)| format!("{c} = {fresh}"))
        .chain(std::iter::once("updated_at = NOW()".to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let stored: Vec<String> = cols.iter().map(|(c, _)| format!("{qi_tv}.{c}")).collect();
    let fresh: Vec<String> = cols.iter().map(|(_, fresh)| fresh.clone()).collect();
    format!("DO UPDATE SET {set} WHERE {}", rows_differ(&stored, &fresh))
}

/// `NOT (ROW(<stored>)::record *= ROW(<fresh>)::record)`: true when the two column
/// lists differ, compared as record images (see [`upsert_conflict_action`]).
///
/// Safe under the owner's `search_path = pg_catalog, pg_temp` for any column type,
/// including types whose `=` lives outside `pg_catalog` or that have none (#156).
pub(crate) fn rows_differ(stored: &[String], fresh: &[String]) -> String {
    format!(
        "NOT (ROW({})::pg_catalog.record OPERATOR(pg_catalog.*=) ROW({})::pg_catalog.record)",
        stored.join(", "),
        fresh.join(", ")
    )
}

/// Run `INSERT INTO qi_tv (col_list) <source_sql> ON CONFLICT (<conflict_key>) <action>`,
/// record the rows its no-op guard skipped (issue #72) and journal the
/// rows it inserted or updated (issue #76).
///
/// The source runs once, in a CTE; the statement returns how many rows the source
/// produced and the `pk_<entity>` of each row written, split into inserted
/// (`xmax = 0`) and updated. The skipped count is added to `refresh_noop_skipped`.
/// Returns how many source rows there were (0: the row is gone from the view).
pub(crate) fn run_counted_upsert(
    entity: &str,
    qi_tv: &str,
    col_list: &str,
    source_sql: &str,
    conflict: &str,
    args: &[DatumWithOid],
) -> spi::Result<i64> {
    let qi_pk = quote_identifier(&format!("pk_{entity}"));
    let sql = format!(
        "WITH src AS ({source_sql}), \
         written AS (INSERT INTO {qi_tv} ({col_list}) SELECT {col_list} FROM src \
                     {conflict} RETURNING {qi_pk}::text AS k, xmax = 0 AS inserted) \
         SELECT (SELECT count(*) FROM src), \
                (SELECT array_agg(k) FROM written WHERE inserted), \
                (SELECT array_agg(k) FROM written WHERE NOT inserted)"
    );
    let (produced, inserted, updated) =
        Spi::get_three_with_args::<i64, Vec<String>, Vec<String>>(&sql, args)?;
    let inserted = inserted.unwrap_or_default();
    let updated = updated.unwrap_or_default();
    let written = (inserted.len() + updated.len()) as u64;
    crate::metrics::metrics_api::record_noop_skipped(
        produced.unwrap_or(0).unsigned_abs().saturating_sub(written),
    );
    for pk in inserted {
        crate::queue::affected::record(entity, pk, crate::queue::affected::Change::Inserted);
    }
    for pk in updated {
        crate::queue::affected::record(entity, pk, crate::queue::affected::Change::Updated);
    }
    Ok(produced.unwrap_or(0))
}

/// Lock the existing rows of `pks` in `qi_tv` (in key order) before recomputing
/// them. Under READ COMMITTED a concurrent writer recomputing one of these rows
/// is then waited for here, and the recompute that follows, a new statement,
/// sees what it committed; without the lock it waited inside its own upsert and
/// wrote a document computed before that commit. Under REPEATABLE READ and
/// SERIALIZABLE the upsert already fails on such a row (SQLSTATE 40001).
pub(crate) fn lock_rows(qi_tv: &str, pk_col: &str, pks: &[i64]) -> spi::Result<()> {
    // SAFETY: reads the backend's isolation level.
    let transaction_snapshot =
        unsafe { pgrx::pg_sys::XactIsoLevel } >= pgrx::pg_sys::XACT_REPEATABLE_READ.cast_signed();
    if transaction_snapshot || pks.is_empty() {
        return Ok(());
    }
    let qi_pk = quote_identifier(pk_col);
    Spi::run_with_args(
        &format!(
            "SELECT 1 FROM {qi_tv} WHERE {qi_pk} OPERATOR(pg_catalog.=) ANY($1) \
             ORDER BY {qi_pk} FOR UPDATE"
        ),
        // SAFETY: the array datum borrows `pks`, which outlives the call.
        &[unsafe { DatumWithOid::new(pks.to_vec(), pgrx::PgBuiltInOids::INT8ARRAYOID.value()) }],
    )
}

/// Journal the rows a `DELETE … RETURNING pk_<entity>::text, id::text` removed.
pub(crate) fn run_journaled_delete(
    entity: &str,
    sql: &str,
    args: &[DatumWithOid],
) -> spi::Result<()> {
    let deleted = Spi::connect_mut(|client| {
        let mut out = Vec::new();
        for row in client.update(sql, None, args)? {
            let pk: Option<String> = row.get(1)?;
            let id: Option<String> = row.get(2)?;
            if let Some(pk) = pk {
                out.push((pk, id));
            }
        }
        Ok::<_, spi::Error>(out)
    })?;
    for (pk, id) in deleted {
        crate::queue::affected::record(entity, pk, crate::queue::affected::Change::Deleted(id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn conflict_action_guards_every_non_key_column() {
        assert_eq!(
            super::upsert_conflict_action(
                r#""app"."tv_post""#,
                &cols(&["pk_post", "id", "data"]),
                "pk_post",
                None
            ),
            r#"DO UPDATE SET "id" = EXCLUDED."id", "data" = EXCLUDED."data", updated_at = NOW() WHERE NOT (ROW("app"."tv_post"."id", "app"."tv_post"."data")::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(EXCLUDED."id", EXCLUDED."data")::pg_catalog.record)"#
        );
    }

    #[test]
    fn conflict_action_single_column_compares_one_column_records() {
        assert_eq!(
            super::upsert_conflict_action(r#""tv_x""#, &cols(&["pk_x", "data"]), "pk_x", None),
            r#"DO UPDATE SET "data" = EXCLUDED."data", updated_at = NOW() WHERE NOT (ROW("tv_x"."data")::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(EXCLUDED."data")::pg_catalog.record)"#
        );
    }

    #[test]
    fn conflict_action_quotes_reserved_and_mixed_case_columns() {
        assert_eq!(
            super::upsert_conflict_action(
                r#""tv_x""#,
                &cols(&["pk_x", "order", "Label"]),
                "pk_x",
                None
            ),
            r#"DO UPDATE SET "order" = EXCLUDED."order", "Label" = EXCLUDED."Label", updated_at = NOW() WHERE NOT (ROW("tv_x"."order", "tv_x"."Label")::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(EXCLUDED."order", EXCLUDED."Label")::pg_catalog.record)"#
        );
    }

    #[test]
    fn conflict_action_data_expr_replaces_only_data() {
        assert_eq!(
            super::upsert_conflict_action(
                r#""tv_x""#,
                &cols(&["pk_x", "label", "data"]),
                "pk_x",
                Some("patch(\"tv_x\".data)"),
            ),
            r#"DO UPDATE SET "label" = EXCLUDED."label", "data" = patch("tv_x".data), updated_at = NOW() WHERE NOT (ROW("tv_x"."label", "tv_x"."data")::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(EXCLUDED."label", patch("tv_x".data))::pg_catalog.record)"#
        );
    }

    #[test]
    fn rows_differ_compares_record_images_qualified() {
        assert_eq!(
            super::rows_differ(&cols(&["t.a", "t.b"]), &cols(&["v.a", "v.b"])),
            "NOT (ROW(t.a, t.b)::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(v.a, v.b)::pg_catalog.record)"
        );
    }

    #[test]
    fn column_list_quotes_every_column() {
        assert_eq!(
            super::column_list(&cols(&["pk_x", "order", "Label", "a\"b"])),
            r#""pk_x", "order", "Label", "a""b""#
        );
    }

    #[test]
    fn conflict_action_key_only_does_nothing() {
        assert_eq!(
            super::upsert_conflict_action(r#""tv_x""#, &cols(&["pk_x"]), "pk_x", None),
            "DO NOTHING"
        );
    }
}

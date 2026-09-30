//! Refresh Module: Smart JSONB Patching for Cascade Updates
//!
//! This module handles refreshing transformed views (TVIEWs) when underlying source
//! table rows change. It uses **smart JSONB patching** via the `jsonb_delta` extension
//! for 1.5-3× performance improvement on cascade updates.

pub mod array_ops;
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
/// updated_at = NOW()` guarded by `IS DISTINCT FROM` over the non-key columns
/// (issue #72).
///
/// A recomputed row that equals the stored one gets no new tuple version, no index
/// entries, no dead tuple, and keeps its `updated_at` ("last content change").
/// `key_col` is the conflict key (excluded from the SET list); target columns are
/// qualified with `tv_name` because the source relation is in scope too.
pub(crate) fn upsert_conflict_action(tv_name: &str, col_names: &[String], key_col: &str) -> String {
    let cols: Vec<String> = col_names
        .iter()
        .filter(|c| c.as_str() != key_col)
        .map(|c| quote_identifier(c))
        .collect();
    if cols.is_empty() {
        return "DO NOTHING".to_string();
    }
    let qi_tv = quote_identifier(tv_name);
    let set = cols
        .iter()
        .map(|c| format!("{c} = EXCLUDED.{c}"))
        .chain(std::iter::once("updated_at = NOW()".to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let (stored, fresh) = if let [c] = cols.as_slice() {
        (format!("{qi_tv}.{c}"), format!("EXCLUDED.{c}"))
    } else {
        (
            format!(
                "({})",
                cols.iter()
                    .map(|c| format!("{qi_tv}.{c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            format!(
                "({})",
                cols.iter()
                    .map(|c| format!("EXCLUDED.{c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    };
    format!("DO UPDATE SET {set} WHERE {stored} IS DISTINCT FROM {fresh}")
}

/// Run `INSERT INTO tv_name (col_list) <source_sql> ON CONFLICT (<conflict_key>) <action>`
/// and record the rows its `IS DISTINCT FROM` guard skipped (issue #72).
///
/// The source runs once, in a CTE; the statement returns how many rows the source
/// produced and how many were inserted or updated, and the difference is added to
/// `refresh_noop_skipped`.
pub(crate) fn run_counted_upsert(
    tv_name: &str,
    col_list: &str,
    source_sql: &str,
    conflict: &str,
    args: &[DatumWithOid],
) -> spi::Result<()> {
    let qi_tv = quote_identifier(tv_name);
    let sql = format!(
        "WITH src AS ({source_sql}), \
         written AS (INSERT INTO {qi_tv} ({col_list}) SELECT {col_list} FROM src \
                     {conflict} RETURNING 1) \
         SELECT (SELECT count(*) FROM src), (SELECT count(*) FROM written)"
    );
    let (produced, written) = Spi::get_two_with_args::<i64, i64>(&sql, args)?;
    let skipped = produced.unwrap_or(0).saturating_sub(written.unwrap_or(0));
    crate::metrics::metrics_api::record_noop_skipped(skipped.unsigned_abs());
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
            super::upsert_conflict_action("tv_post", &cols(&["pk_post", "id", "data"]), "pk_post"),
            r#"DO UPDATE SET "id" = EXCLUDED."id", "data" = EXCLUDED."data", updated_at = NOW() WHERE ("tv_post"."id", "tv_post"."data") IS DISTINCT FROM (EXCLUDED."id", EXCLUDED."data")"#
        );
    }

    #[test]
    fn conflict_action_single_column_uses_plain_comparison() {
        assert_eq!(
            super::upsert_conflict_action("tv_x", &cols(&["pk_x", "data"]), "pk_x"),
            r#"DO UPDATE SET "data" = EXCLUDED."data", updated_at = NOW() WHERE "tv_x"."data" IS DISTINCT FROM EXCLUDED."data""#
        );
    }

    #[test]
    fn conflict_action_quotes_reserved_and_mixed_case_columns() {
        assert_eq!(
            super::upsert_conflict_action("tv_x", &cols(&["pk_x", "order", "Label"]), "pk_x"),
            r#"DO UPDATE SET "order" = EXCLUDED."order", "Label" = EXCLUDED."Label", updated_at = NOW() WHERE ("tv_x"."order", "tv_x"."Label") IS DISTINCT FROM (EXCLUDED."order", EXCLUDED."Label")"#
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
            super::upsert_conflict_action("tv_x", &cols(&["pk_x"]), "pk_x"),
            "DO NOTHING"
        );
    }
}

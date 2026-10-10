//! Writing TVIEW rows: one row recomputed from its backing view (`row`), many at
//! once (`bulk`), and the direct patch of a TVIEW's own columns (`direct`).

pub mod row;

pub mod bulk;
pub mod direct;

pub use bulk::refresh_bulk;
pub use row::refresh_key;

use crate::catalog::KeyType;
use crate::queue::key::KeyValue;
use crate::utils::quote_identifier;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// `param` (`$1`) cast to the identity's type, or to an array of it: an integer
/// identity is bound as `int8`, any other as text cast to its type, so a filter on
/// the identity column can use its index.
pub(crate) fn key_cast(key_type: &KeyType, param: &str, array: bool) -> String {
    let brackets = if array { "[]" } else { "" };
    match key_type {
        KeyType::Int => format!("{param}::pg_catalog.int8{brackets}"),
        KeyType::Text(ty) => format!("{param}::pg_catalog.text{brackets}::{ty}{brackets}"),
    }
}

/// The values of `keys` as the identity binds them; a text key of an integer
/// identity that is not an integer is an error.
fn key_values(key_type: &KeyType, keys: &[KeyValue]) -> crate::TViewResult<KeyValues> {
    Ok(match key_type {
        KeyType::Int => KeyValues::Int(
            keys.iter()
                .map(|k| {
                    k.to_int().ok_or_else(|| crate::TViewError::InvalidInput {
                        parameter: "key".to_string(),
                        reason: format!("{k} is not a value of an integer identity"),
                    })
                })
                .collect::<crate::TViewResult<_>>()?,
        ),
        KeyType::Text(_) => KeyValues::Text(keys.iter().map(ToString::to_string).collect()),
    })
}

enum KeyValues {
    Int(Vec<i64>),
    Text(Vec<String>),
}

/// `keys` as one array parameter, for `key_cast(…, true)`.
pub(crate) fn key_array(
    key_type: &KeyType,
    keys: &[KeyValue],
) -> crate::TViewResult<DatumWithOid<'static>> {
    Ok(match key_values(key_type, keys)? {
        KeyValues::Int(v) => crate::utils::spi::int8_array(v),
        KeyValues::Text(v) => crate::utils::spi::text_array(v),
    })
}

/// One key as a parameter, for `key_cast(…, false)`.
pub(crate) fn key_scalar(
    key_type: &KeyType,
    key: &KeyValue,
) -> crate::TViewResult<DatumWithOid<'static>> {
    Ok(match key_values(key_type, std::slice::from_ref(key))? {
        KeyValues::Int(v) => crate::utils::spi::int8(v[0]),
        KeyValues::Text(mut v) => crate::utils::spi::text(v.swap_remove(0)),
    })
}

/// The rows of a TVIEW a refresh touched, by the `pk_<entity>` values parents look
/// them up by (ADR 0169, D4).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Touched {
    /// Every row refreshed, before and after.
    pub pks: Vec<i64>,
    /// The rows that were not in the table before: a parent that embeds them
    /// through an inner join may be missing from its own table too.
    pub appeared: Vec<i64>,
}

impl Touched {
    pub fn extend(&mut self, other: Self) {
        self.pks.extend(other.pks);
        self.appeared.extend(other.appeared);
    }
}

/// What the refresh of `keys` touched: when the identity is `pk_<entity>`, the
/// keys themselves and the rows inserted; otherwise the rows' `pk_<entity>` before
/// the refresh (`before`) and those it wrote or deleted, of which the written ones
/// not there before appeared.
pub(crate) fn touched(
    meta: &crate::catalog::TviewMeta,
    keys: &[KeyValue],
    before: Vec<i64>,
    written: Written,
    deleted: Vec<i64>,
) -> Touched {
    if meta.identity.is_pk(&meta.entity_name) {
        return Touched {
            // A key read off a column of another type (a domain, numeric) is text.
            pks: keys
                .iter()
                .filter_map(|k| match k {
                    KeyValue::Int(v) => Some(*v),
                    KeyValue::Text(t) => t.parse().ok(),
                })
                .collect(),
            appeared: written.inserted,
        };
    }
    let appeared: Vec<i64> = written
        .inserted
        .iter()
        .chain(&written.updated)
        .copied()
        .filter(|pk| !before.contains(pk))
        .collect();
    let mut pks = before;
    pks.extend(written.inserted);
    pks.extend(written.updated);
    pks.extend(deleted);
    pks.sort_unstable();
    pks.dedup();
    Touched { pks, appeared }
}

/// The `pk_<entity>` of the rows an upsert inserted and updated.
#[derive(Debug, Default)]
pub(crate) struct Written {
    pub inserted: Vec<i64>,
    pub updated: Vec<i64>,
}

/// Quoted, comma-separated column list of a refresh upsert (`INSERT INTO tv (…)`
/// and the matching `SELECT …`), so reserved-word and mixed-case columns work.
pub(crate) fn column_list(col_names: &[String]) -> String {
    col_names
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `ON CONFLICT` action for a refresh upsert: `DO UPDATE SET <cols> = EXCLUDED.<cols>,
/// updated_at = NOW()` guarded by a comparison of the non-key columns.
///
/// The guard compares record images ([`rows_differ`]), with the operator and type
/// qualified. The flush runs under the owner's
/// `search_path = pg_catalog, pg_temp`, so a per-type `=` installed elsewhere
/// (ltree, citext, hstore) would not be found, and some types (json, point) have no
/// `=` at all. `*=` needs neither. The `::record` casts stop the parser from
/// expanding `ROW(..) op ROW(..)` into one per-column `*=`. Equality is binary: NULL
/// equals NULL, but citext `'A'` vs `'a'` or numeric `1.0` vs `1.00` count as changes,
/// which is what a materialized copy should record.
///
/// A recomputed row that equals the stored one gets no new tuple version, no index
/// entries, no dead tuple, and keeps its `updated_at` ("last content change").
/// `key_col` is the conflict key (excluded from the SET list); target columns are
/// qualified with `qi_tv`, the quoted (schema-qualified) TVIEW table, because the
/// source relation is in scope too.
pub(crate) fn upsert_conflict_action(qi_tv: &str, col_names: &[String], key_col: &str) -> String {
    let cols: Vec<(String, String)> = col_names
        .iter()
        .filter(|c| c.as_str() != key_col)
        .map(|c| {
            let q = quote_identifier(c);
            let fresh = format!("EXCLUDED.{q}");
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
/// including types whose `=` lives outside `pg_catalog` or that have none.
pub(crate) fn rows_differ(stored: &[String], fresh: &[String]) -> String {
    format!(
        "NOT (ROW({})::pg_catalog.record OPERATOR(pg_catalog.*=) ROW({})::pg_catalog.record)",
        stored.join(", "),
        fresh.join(", ")
    )
}

/// Run `INSERT INTO qi_tv (col_list) <source_sql> ON CONFLICT (<conflict_key>) <action>`,
/// record the rows its no-op guard skipped and journal the
/// rows it inserted or updated.
///
/// The source runs once, in a CTE; the statement returns how many rows the source
/// produced and the `pk_<entity>` of each row written, split into inserted
/// (`xmax = 0`) and updated. The skipped count is added to `refresh_noop_skipped`.
/// Returns how many source rows there were (0: the row is gone from the view), and
/// the rows written.
pub(crate) fn run_counted_upsert(
    entity: &str,
    qi_tv: &str,
    col_list: &str,
    source_sql: &str,
    conflict: &str,
    args: &[DatumWithOid],
) -> crate::TViewResult<(i64, Written)> {
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
    let parse = |pks: &[String]| pks.iter().filter_map(|pk| pk.parse().ok()).collect();
    let written = Written {
        inserted: parse(&inserted),
        updated: parse(&updated),
    };
    for pk in inserted {
        crate::queue::affected::record(entity, pk, crate::queue::affected::Change::Inserted);
    }
    for pk in updated {
        crate::queue::affected::record(entity, pk, crate::queue::affected::Change::Updated);
    }
    Ok((produced.unwrap_or(0), written))
}

/// Lock the existing rows of `keys` in the TVIEW (in key order) before
/// recomputing them, and the keys of those it doesn't hold yet, and return the
/// existing rows' `pk_<entity>`. Under READ COMMITTED a concurrent writer
/// recomputing one of these rows, or creating it, is then waited for here, and
/// the recompute that follows, a new statement, sees what it committed; without
/// the lock it waited inside its own upsert and wrote a document computed before
/// that commit. Under REPEATABLE READ and SERIALIZABLE the upsert already fails
/// on an existing row (SQLSTATE 40001): those rows are only read; a key another
/// transaction is creating fails at once (REPEATABLE READ, ADR 0207).
pub(crate) fn lock_rows(
    meta: &crate::catalog::TviewMeta,
    qi_tv: &str,
    keys: &[KeyValue],
    with_pks: bool,
) -> crate::TViewResult<Vec<i64>> {
    use crate::concurrency::Policy;
    // SAFETY: reads the backend's isolation level.
    let transaction_snapshot =
        unsafe { pgrx::pg_sys::XactIsoLevel } >= pgrx::pg_sys::XACT_REPEATABLE_READ.cast_signed();
    let key_locks = Policy::current() != Policy::Skip;
    if keys.is_empty() || (transaction_snapshot && !with_pks && !key_locks) {
        return Ok(Vec::new());
    }
    let key_type = meta.key_type()?;
    let qi_key = quote_identifier(&meta.identity.column);
    let qi_pk = quote_identifier(&format!("pk_{}", meta.entity_name));
    let lock = if transaction_snapshot {
        ""
    } else {
        " FOR UPDATE"
    };
    let sql = format!(
        "SELECT t.{qi_pk}::pg_catalog.int8 AS pk, t.{qi_key}::pg_catalog.text AS key \
         FROM {qi_tv} t WHERE t.{qi_key} OPERATOR(pg_catalog.=) ANY({}) ORDER BY t.{qi_key}{lock}",
        key_cast(&key_type, "$1", true)
    );
    let args = [key_array(&key_type, keys)?];
    // Read-write: a read-only SPI call refuses FOR UPDATE.
    let (pks, held) = Spi::connect_mut(|client| {
        let mut pks = Vec::new();
        let mut held = std::collections::HashSet::new();
        for row in client.update(&sql, None, &args)? {
            if let Some(pk) = row.get::<i64>(1)? {
                pks.push(pk);
            }
            held.extend(row.get::<String>(2)?);
        }
        Ok::<_, spi::Error>((pks, held))
    })?;
    if key_locks {
        // As the identity's type writes them, as the rows found are compared:
        // the same uuid can be queued in another spelling.
        let keys: Vec<String> = match &key_type {
            KeyType::Int => keys.iter().map(ToString::to_string).collect(),
            KeyType::Text(_) => crate::utils::spi::strings(
                &format!(
                    "SELECT k::pg_catalog.text FROM pg_catalog.unnest({}) k",
                    key_cast(&key_type, "$1", true)
                ),
                &args,
            )?,
        };
        let missing: Vec<String> = keys.into_iter().filter(|k| !held.contains(k)).collect();
        crate::concurrency::lock_new_keys(meta.tview_oid, &missing);
    }
    Ok(pks)
}

/// Journal the rows a `DELETE … RETURNING pk_<entity>::text, id::text` removed,
/// and return their `pk_<entity>`.
pub(crate) fn run_journaled_delete(
    entity: &str,
    sql: &str,
    args: &[DatumWithOid],
) -> crate::TViewResult<Vec<i64>> {
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
    let pks = deleted
        .iter()
        .filter_map(|(pk, _)| pk.parse().ok())
        .collect();
    for (pk, id) in deleted {
        crate::queue::affected::record(entity, pk, crate::queue::affected::Change::Deleted(id));
    }
    Ok(pks)
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
                "pk_post"
            ),
            r#"DO UPDATE SET "id" = EXCLUDED."id", "data" = EXCLUDED."data", updated_at = NOW() WHERE NOT (ROW("app"."tv_post"."id", "app"."tv_post"."data")::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(EXCLUDED."id", EXCLUDED."data")::pg_catalog.record)"#
        );
    }

    #[test]
    fn conflict_action_single_column_compares_one_column_records() {
        assert_eq!(
            super::upsert_conflict_action(r#""tv_x""#, &cols(&["pk_x", "data"]), "pk_x"),
            r#"DO UPDATE SET "data" = EXCLUDED."data", updated_at = NOW() WHERE NOT (ROW("tv_x"."data")::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(EXCLUDED."data")::pg_catalog.record)"#
        );
    }

    #[test]
    fn conflict_action_quotes_reserved_and_mixed_case_columns() {
        assert_eq!(
            super::upsert_conflict_action(r#""tv_x""#, &cols(&["pk_x", "order", "Label"]), "pk_x"),
            r#"DO UPDATE SET "order" = EXCLUDED."order", "Label" = EXCLUDED."Label", updated_at = NOW() WHERE NOT (ROW("tv_x"."order", "tv_x"."Label")::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(EXCLUDED."order", EXCLUDED."Label")::pg_catalog.record)"#
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
    fn key_cast_binds_int8_or_text_cast_to_the_type() {
        use crate::catalog::KeyType;
        assert_eq!(
            super::key_cast(&KeyType::Int, "$1", true),
            "$1::pg_catalog.int8[]"
        );
        assert_eq!(
            super::key_cast(&KeyType::Int, "$2", false),
            "$2::pg_catalog.int8"
        );
        let uuid = KeyType::Text("pg_catalog.uuid".into());
        assert_eq!(
            super::key_cast(&uuid, "$1", true),
            "$1::pg_catalog.text[]::pg_catalog.uuid[]"
        );
        assert_eq!(
            super::key_cast(&uuid, "$1", false),
            "$1::pg_catalog.text::pg_catalog.uuid"
        );
    }

    #[test]
    fn conflict_action_key_only_does_nothing() {
        assert_eq!(
            super::upsert_conflict_action(r#""tv_x""#, &cols(&["pk_x"]), "pk_x"),
            "DO NOTHING"
        );
    }
}

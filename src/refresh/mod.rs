//! Writing TVIEW rows: one row recomputed from its backing view (`row`), many at
//! once (`bulk`), and the direct patch of a TVIEW's own columns (`direct`).

pub mod row;

pub mod bulk;
pub mod direct;
pub mod full;

pub use bulk::refresh_bulk;
pub use row::refresh_key;

use crate::catalog::KeyType;
use crate::queue::key::KeyValue;
use crate::utils::ident;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use std::fmt::Write as _;

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

/// The identities an upsert's source produced ([`Produced::keys`], as text), as
/// one array parameter for `key_cast(…, true)`.
pub(crate) fn produced_keys(
    key_type: &KeyType,
    keys: Vec<String>,
) -> crate::TViewResult<DatumWithOid<'static>> {
    Ok(match key_type {
        KeyType::Int => crate::utils::spi::int8_array(
            keys.iter()
                .map(|k| {
                    k.parse().map_err(|_| crate::TViewError::InvalidInput {
                        parameter: "key".to_string(),
                        reason: format!("{k} is not a value of an integer identity"),
                    })
                })
                .collect::<crate::TViewResult<_>>()?,
        ),
        KeyType::Text(_) => crate::utils::spi::text_array(keys),
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

/// Identity keys bound as `$1` for a lookup, shaped by their count as
/// [`crate::utils::spi::LookupKeys`] shapes keys of `fk_<entity>`: a backing view
/// read by identity may join many rows of a table for each key (an aggregate of
/// children), which a generic plan for an array estimates as ten keys' worth.
pub(crate) struct KeyLookup {
    /// What the identity is compared with by `OPERATOR(pg_catalog.=)`.
    pub rhs: String,
    /// The keys, as `rhs` reads them.
    pub arg: DatumWithOid<'static>,
    /// How a statement looking them up is planned.
    pub plan: crate::utils::spi::Plan,
}

/// The lookup of `keys` of an identity of `key_type`; `None` without any.
///
/// # Errors
/// A text key of an integer identity that is not an integer.
pub(crate) fn key_lookup(
    key_type: &KeyType,
    keys: &[KeyValue],
) -> crate::TViewResult<Option<KeyLookup>> {
    Ok(match keys {
        [] => None,
        [key] => Some(KeyLookup {
            rhs: key_cast(key_type, "$1", false),
            arg: key_scalar(key_type, key)?,
            plan: crate::utils::spi::Plan::Generic,
        }),
        _ => Some(KeyLookup {
            rhs: format!("ANY ({})", key_cast(key_type, "$1", true)),
            arg: key_array(key_type, keys)?,
            plan: crate::utils::spi::Plan::PerExecution,
        }),
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

/// Refuse a UNION TVIEW whose backing view returns several rows for one key
/// (ADR 0216), among the rows `filter` selects (`$1`… bound to `args`): a key
/// names one row, and no path picks one of several. Nothing to check for a
/// TVIEW without a set operation.
///
/// # Errors
/// [`TViewError::DuplicateKey`](crate::TViewError::DuplicateKey) for the first
/// such key, or the error of the query.
pub(crate) fn refuse_duplicate_keys(
    meta: &crate::catalog::TviewMeta,
    filter: &str,
    args: &[DatumWithOid],
) -> crate::TViewResult<()> {
    if !meta.plan.set_operation {
        return Ok(());
    }
    refuse_duplicate_keys_in(
        &crate::utils::qualified_relname_from_oid(meta.tview_oid)?,
        &meta.identity.column,
        &crate::utils::qualified_relname_from_oid(meta.view_oid)?,
        filter,
        args,
    )
}

/// [`refuse_duplicate_keys`] for a TVIEW not registered yet: its table
/// `qualified_tv`, key column and backing view `qualified_view`.
///
/// # Errors
/// As [`refuse_duplicate_keys`].
pub(crate) fn refuse_duplicate_keys_in(
    qualified_tv: &str,
    key_column: &str,
    qualified_view: &str,
    filter: &str,
    args: &[DatumWithOid],
) -> crate::TViewResult<()> {
    let qi_key = ident::quoted(key_column);
    let sql = format!(
        "SELECT (SELECT {qi_key}::pg_catalog.text FROM {qualified_view} WHERE {filter} \
                 GROUP BY {qi_key} HAVING pg_catalog.count(*) > 1 LIMIT 1)"
    );
    let duplicate = Spi::connect(|client| {
        crate::utils::spi::kept_select(client, &sql, args)?
            .first()
            .get::<String>(1)
    })
    .map_err(|e| crate::utils::spi::error(&sql, &e))?;
    match duplicate {
        Some(key) => Err(crate::TViewError::DuplicateKey {
            tview: qualified_tv.to_string(),
            key_column: key_column.to_string(),
            key,
        }),
        None => Ok(()),
    }
}

/// Quoted, comma-separated column list of a refresh upsert (`INSERT INTO tv (…)`
/// and the matching `SELECT …`), so reserved-word and mixed-case columns work.
pub(crate) fn column_list(col_names: &[String]) -> String {
    col_names
        .iter()
        .map(|c| ident::quoted(c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `ON CONFLICT` action for a refresh upsert: `DO UPDATE SET <cols> = EXCLUDED.<cols>`
/// guarded by a comparison of the non-key columns.
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
/// entries and no dead tuple.
/// `key_col` is the conflict key (excluded from the SET list); target columns are
/// qualified with `qi_tv`, the quoted (schema-qualified) TVIEW table, because the
/// source relation is in scope too.
pub(crate) fn upsert_conflict_action(qi_tv: &str, col_names: &[String], key_col: &str) -> String {
    let cols: Vec<(String, String)> = col_names
        .iter()
        .filter(|c| c.as_str() != key_col)
        .map(|c| {
            let q = ident::quoted(c);
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

/// What a refresh upsert's source produced, and what it wrote.
#[derive(Debug, Default)]
pub(crate) struct Produced {
    /// The identities of the rows the source produced, as text (none: the keys
    /// are gone from the view).
    pub keys: Vec<String>,
    /// The rows written.
    pub written: Written,
}

/// Run `INSERT INTO qi_tv (col_list) <source_sql> ON CONFLICT (<conflict_key>) <action>`,
/// record the rows its no-op guard skipped and journal the
/// rows it inserted or updated.
///
/// The source runs once, in a CTE; the statement returns the identities
/// (`key_col`) of the rows the source produced and the `pk_<entity>` of each row
/// written, split into inserted (`xmax = 0`) and updated. The skipped count is
/// added to `refresh_noop_skipped`. `old` holds the reference keys of the rows
/// [`lock_rows`] read; without it the statement reads them itself (a pre-image).
pub(crate) fn run_counted_upsert(
    meta: &crate::catalog::TviewMeta,
    qi_tv: &str,
    col_list: &str,
    source_sql: &str,
    conflict: &str,
    args: &[DatumWithOid],
    old: Option<&crate::catalog::collections::OldKeys>,
) -> crate::TViewResult<Produced> {
    use crate::queue::affected::{Change, Recorded, record};
    let entity = &meta.entity_name;
    let qi_pk = ident::quoted(&format!("pk_{entity}"));
    let qi_key = ident::quoted(&meta.identity.column);
    // A UNION view returning several rows for one key is refused (ADR 0216) from
    // the rows the upsert reads, which then writes none: the view is read once.
    let (duplicate, unless_duplicate, duplicate_key) = if meta.plan.set_operation {
        (
            format!(
                "dup AS (SELECT {qi_key}::pg_catalog.text AS key FROM src \
                         GROUP BY {qi_key} HAVING pg_catalog.count(*) > 1 LIMIT 1), "
            ),
            "WHERE NOT EXISTS (SELECT FROM dup)",
            "(SELECT key FROM dup)",
        )
    } else {
        (String::new(), "", "NULL::pg_catalog.text")
    };
    let sql = format!(
        "WITH src AS ({source_sql}), {duplicate}\
         written AS (INSERT INTO {qi_tv} ({col_list}) SELECT {col_list} FROM src {unless_duplicate} \
                     {conflict} RETURNING {qi_pk}::pg_catalog.int8 AS k, {} AS i, {} AS r, \
                                          xmax = 0 AS inserted, \
                                          {qi_key}::pg_catalog.text AS key) \
         SELECT (SELECT array_agg({qi_key}::pg_catalog.text) FROM src), \
                (SELECT pg_catalog.jsonb_agg(pg_catalog.jsonb_build_array(k, i, inserted, r, key)) \
                 FROM written), {duplicate_key}",
        meta.id_text(qi_tv),
        meta.reference_values(qi_tv, old.is_none())
    );
    let (keys, rows, duplicate) = Spi::connect_mut(|client| {
        let row = crate::utils::spi::kept_update(client, &sql, args)?.first();
        Ok::<_, spi::Error>((
            row.get::<Vec<String>>(1)?,
            row.get::<pgrx::JsonB>(2)?,
            row.get::<String>(3)?,
        ))
    })?;
    if let Some(key) = duplicate {
        return Err(crate::TViewError::DuplicateKey {
            tview: qi_tv.to_string(),
            key_column: meta.identity.column.clone(),
            key,
        });
    }
    let keys = keys.unwrap_or_default();
    let rows = match rows {
        Some(pgrx::JsonB(serde_json::Value::Array(rows))) => rows,
        _ => Vec::new(),
    };
    crate::metrics::metrics_api::record_noop_skipped(
        entity,
        keys.len().saturating_sub(rows.len()) as u64,
    );
    let mut written = Written::default();
    let mut created = Vec::new();
    for row in rows {
        let (Some(pk), id, Some(inserted)) = (
            row[0].as_i64(),
            row[1].as_str().map(str::to_string),
            row[2].as_bool(),
        ) else {
            continue;
        };
        if inserted {
            written.inserted.push(pk);
            created.extend(row[4].as_str().map(str::to_string));
        } else {
            written.updated.push(pk);
        }
        let change = if inserted {
            Change::Inserted
        } else {
            Change::Updated
        };
        let references: Vec<Option<String>> = row[3]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let held = old.and_then(|o| o.get(&pk)).map(Vec::as_slice);
        record(Recorded::of(meta, pk, id, change, &references, held));
    }
    refuse_unlocked_creations(meta, qi_tv, &created)?;
    Ok(Produced { keys, written })
}

/// A refresh creates a TVIEW row only under its key lock (ADR 0207): every other
/// transaction creating the same row waits for it, and one that finds the row
/// then locks it before computing ([`lock_rows`]). A row created without the
/// lock is a key whose row the refresh found, gone since: under REPEATABLE READ
/// and SERIALIZABLE, a row a concurrent transaction deleted after the snapshot,
/// which fails with 40001 as an UPDATE of that row would. Under READ COMMITTED
/// the found rows are locked, so it can't happen.
///
/// # Errors
/// An internal error for a row created without its key lock under READ
/// COMMITTED.
fn refuse_unlocked_creations(
    meta: &crate::catalog::TviewMeta,
    qi_tv: &str,
    created: &[String],
) -> crate::TViewResult<()> {
    if created
        .iter()
        .all(|key| crate::concurrency::holds_new_key(meta.tview_oid, key))
    {
        return Ok(());
    }
    if crate::concurrency::crosscheck::enabled() {
        crate::concurrency::serialization_failure(
            "a concurrent transaction deleted a TVIEW row this refresh read",
        );
    }
    Err(crate::TViewError::CatalogError {
        operation: format!("Create a row of {qi_tv}"),
        pg_error: "the row was created without its key lock".to_string(),
    })
}

/// Lock the existing rows of `keys` in the TVIEW (in key order) before
/// recomputing them, then the keys of those it doesn't hold yet, then the rows
/// of those keys created in between, and return the rows' `pk_<entity>`. Under READ COMMITTED a concurrent writer
/// recomputing one of these rows, or creating it, is then waited for here, and
/// the recompute that follows, a new statement, sees what it committed; without
/// the lock it waited inside its own upsert and wrote a document computed before
/// that commit. Under REPEATABLE READ and SERIALIZABLE the upsert already fails
/// on an existing row (SQLSTATE 40001): those rows are only read; a key another
/// transaction is creating fails at once (ADR 0207).
pub(crate) fn lock_rows(
    meta: &crate::catalog::TviewMeta,
    qi_tv: &str,
    keys: &[KeyValue],
) -> crate::TViewResult<Locked> {
    // SAFETY: reads the backend's isolation level.
    let transaction_snapshot =
        unsafe { pgrx::pg_sys::XactIsoLevel } >= pgrx::pg_sys::XACT_REPEATABLE_READ.cast_signed();
    if keys.is_empty() {
        return Ok(Locked::default());
    }
    let key_type = &meta.key_type;
    let qi_key = ident::quoted(&meta.identity.column);
    let qi_pk = ident::quoted(&format!("pk_{}", meta.entity_name));
    let lock = if transaction_snapshot {
        ""
    } else {
        " FOR UPDATE"
    };
    // The reference keys the rows hold before the recompute: what the change
    // feed names as the collections a row leaves.
    let references = meta.publishes_collections();
    let sql = format!(
        "SELECT t.{qi_pk}::pg_catalog.int8 AS pk, t.{qi_key}::pg_catalog.text AS key, {} \
         FROM {qi_tv} t WHERE t.{qi_key} OPERATOR(pg_catalog.=) ANY({}) ORDER BY t.{qi_key}{lock}",
        meta.reference_keys("t"),
        key_cast(key_type, "$1", true)
    );
    let args = [key_array(key_type, keys)?];
    let (mut pks, held, mut old) = locked_rows(&sql, &args, references)?;
    // As the identity's type writes them, as the rows found are compared:
    // the same uuid can be queued in another spelling.
    let keys: Vec<String> = match &key_type {
        KeyType::Int => keys.iter().map(ToString::to_string).collect(),
        KeyType::Text(_) => crate::utils::spi::strings(
            &format!(
                "SELECT k::pg_catalog.text FROM pg_catalog.unnest({}) k",
                key_cast(key_type, "$1", true)
            ),
            &args,
        )?,
    };
    let missing: Vec<String> = keys.into_iter().filter(|k| !held.contains(k)).collect();
    crate::concurrency::lock_new_keys(meta.tview_oid, &missing);
    // A missing row another transaction created and committed since the first
    // statement, and a third one may hold now, is locked too, in a new
    // statement: the key lock only stops new creations of it. Without this the
    // upsert waits for that row's lock inside ON CONFLICT DO UPDATE and writes
    // what it computed before the wait.
    if !transaction_snapshot && !missing.is_empty() {
        let created: Vec<KeyValue> = missing.into_iter().map(KeyValue::Text).collect();
        let args = [key_array(key_type, &created)?];
        let (more, _, more_old) = locked_rows(&sql, &args, references)?;
        pks.extend(more);
        old.extend(more_old);
    }
    Ok(Locked {
        pks,
        old: Some(old),
    })
}

/// The rows `sql` (from [`lock_rows`]) returns: their `pk_<entity>`, their keys
/// as text, and, for `references`, the reference keys they hold.
fn locked_rows(
    sql: &str,
    args: &[pgrx::datum::DatumWithOid<'_>],
    references: bool,
) -> crate::TViewResult<(
    Vec<i64>,
    std::collections::HashSet<String>,
    crate::catalog::collections::OldKeys,
)> {
    // Read-write: a read-only SPI call refuses FOR UPDATE.
    Ok(Spi::connect_mut(|client| {
        let mut pks = Vec::new();
        let mut held = std::collections::HashSet::new();
        let mut old = crate::catalog::collections::OldKeys::new();
        for row in crate::utils::spi::kept_update(client, sql, args)? {
            if let Some(pk) = row.get::<i64>(1)? {
                if references {
                    old.insert(pk, row.get::<Vec<Option<String>>>(3)?.unwrap_or_default());
                }
                pks.push(pk);
            }
            held.extend(row.get::<String>(2)?);
        }
        Ok::<_, spi::Error>((pks, held, old))
    })?)
}

/// Lock the existing rows of `keys` in `meta`'s TVIEW in key order, and read
/// `columns` (integer lookup columns) off the version locked, the latest
/// committed one, by `pk_<entity>`. A refresh that writes rows in several
/// statements (direct patches, grouped by shape, then recomputes) then takes
/// their row locks in the order every concurrent refresh does, so two of them
/// never wait for each other in a cycle (#263); and a patch written into a parent
/// can be checked against what the parent embeds now. Under REPEATABLE READ and
/// SERIALIZABLE a row updated since the snapshot fails the write here (40001), as
/// it would in the refresh; one being updated is waited for in key order too.
///
/// # Errors
/// Returns an error if the rows cannot be locked.
pub(crate) fn lock_in_key_order(
    meta: &crate::catalog::TviewMeta,
    keys: &[KeyValue],
    columns: &[String],
) -> crate::TViewResult<std::collections::HashMap<i64, Vec<Option<i64>>>> {
    let key_type = &meta.key_type;
    let qi_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qi_key = ident::quoted(&meta.identity.column);
    let qi_pk = ident::quoted(&format!("pk_{}", meta.entity_name));
    let mut read = String::new();
    for column in columns {
        let _ = write!(read, ", t.{}::pg_catalog.int8", ident::quoted(column));
    }
    let sql = format!(
        "SELECT t.{qi_pk}::pg_catalog.int8{read} FROM {qi_tv} t \
         WHERE t.{qi_key} OPERATOR(pg_catalog.=) ANY({}) ORDER BY t.{qi_key} FOR UPDATE",
        key_cast(key_type, "$1", true)
    );
    let args = [key_array(key_type, keys)?];
    // Read-write: a read-only SPI call refuses FOR UPDATE.
    let rows = Spi::connect_mut(|client| {
        let mut rows = std::collections::HashMap::new();
        for row in crate::utils::spi::kept_update(client, &sql, &args)? {
            let Some(pk) = row.get::<i64>(1)? else {
                continue;
            };
            let mut values = Vec::with_capacity(columns.len());
            for i in 0..columns.len() {
                values.push(row.get::<i64>(i + 2)?);
            }
            rows.insert(pk, values);
        }
        Ok::<_, spi::Error>(rows)
    })?;
    Ok(rows)
}

/// What [`lock_rows`] read of the rows it locked.
#[derive(Debug, Default)]
pub(crate) struct Locked {
    /// The existing rows' `pk_<entity>`.
    pub pks: Vec<i64>,
    /// Their reference keys, when the change feed names collections (empty
    /// otherwise); `None` when no key was given, so a recompute reads them
    /// itself.
    pub old: Option<crate::catalog::collections::OldKeys>,
}

/// Run `sql`, a write of `meta`'s TVIEW `RETURNING pk_<entity>::int8` and what
/// [`crate::catalog::TviewMeta::returned`] lists, journal each row it returns as
/// `change`, and return their `pk_<entity>`.
pub(crate) fn run_journaled(
    meta: &crate::catalog::TviewMeta,
    sql: &str,
    args: &[DatumWithOid],
    change: crate::queue::affected::Change,
) -> crate::TViewResult<Vec<i64>> {
    let written = Spi::connect_mut(|client| {
        let mut out = Vec::new();
        for row in crate::utils::spi::kept_update(client, sql, args)? {
            let pk: Option<i64> = row.get(1)?;
            let id: Option<String> = row.get(2)?;
            let references: Option<Vec<Option<String>>> = row.get(3)?;
            if let Some(pk) = pk {
                out.push((pk, id, references.unwrap_or_default()));
            }
        }
        Ok::<_, spi::Error>(out)
    })?;
    Ok(written
        .into_iter()
        .map(|(pk, id, references)| {
            crate::queue::affected::record(crate::queue::affected::Recorded::of(
                meta,
                pk,
                id,
                change,
                &references,
                None,
            ));
            pk
        })
        .collect())
}

/// [`run_journaled`] for a DELETE: the `pk_<entity>` of the rows it removed.
pub(crate) fn run_journaled_delete(
    meta: &crate::catalog::TviewMeta,
    sql: &str,
    args: &[DatumWithOid],
) -> crate::TViewResult<Vec<i64>> {
    run_journaled(meta, sql, args, crate::queue::affected::Change::Deleted)
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
            r#"DO UPDATE SET "id" = EXCLUDED."id", "data" = EXCLUDED."data" WHERE NOT (ROW("app"."tv_post"."id", "app"."tv_post"."data")::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(EXCLUDED."id", EXCLUDED."data")::pg_catalog.record)"#
        );
    }

    #[test]
    fn conflict_action_single_column_compares_one_column_records() {
        assert_eq!(
            super::upsert_conflict_action(r#""tv_x""#, &cols(&["pk_x", "data"]), "pk_x"),
            r#"DO UPDATE SET "data" = EXCLUDED."data" WHERE NOT (ROW("tv_x"."data")::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(EXCLUDED."data")::pg_catalog.record)"#
        );
    }

    #[test]
    fn conflict_action_quotes_reserved_and_mixed_case_columns() {
        assert_eq!(
            super::upsert_conflict_action(r#""tv_x""#, &cols(&["pk_x", "order", "Label"]), "pk_x"),
            r#"DO UPDATE SET "order" = EXCLUDED."order", "Label" = EXCLUDED."Label" WHERE NOT (ROW("tv_x"."order", "tv_x"."Label")::pg_catalog.record OPERATOR(pg_catalog.*=) ROW(EXCLUDED."order", EXCLUDED."Label")::pg_catalog.record)"#
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

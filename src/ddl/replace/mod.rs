//! `pg_tviews_create_or_replace()`: create a TVIEW, or bring an existing one to a
//! definition and storage options with the smallest change (ADR 0136
//! Decision 5).
//!
//! The options passed are the whole declaration: an option not passed is at its
//! default (ADR 0220), whatever the TVIEW had.
//!
//! - **created**: the TVIEW did not exist.
//! - **unchanged**: the definition and the options match what exists.
//! - **altered**: only options other than `group_keys` differ; changed in place,
//!   rows kept.
//! - **replaced**: the definition differs but produces the same columns, and
//!   `group_keys` is the same; the backing view is replaced and the rows reconciled
//!   in place, touching only rows that change.
//! - **rebuilt**: the columns or `group_keys` differ; the TVIEW is dropped and
//!   created again, with its owner, privileges, comment and user indexes carried
//!   over. Refused when something depends on it or it has what a
//!   rebuild cannot carry.
//!
//! The DDL runs as the caller; replacing an existing TVIEW requires owning it.

use super::create;
use super::uncascaded::{Declarations, TimeRefresh};
use crate::catalog::TviewMeta;
use crate::catalog::resolve::{self, Name};
use crate::error::{TViewError, TViewResult};
use crate::utils::ident;
use pgrx::prelude::*;

mod options;
mod rebuild;

use options::invalid;
pub(crate) use options::{Declared, Options, parse_options};
use rebuild::{alter_storage, rebuild};

/// Create `name` from `query`, or bring the existing TVIEW to it.
///
/// # Errors
/// Returns an error for invalid options or names, a definition that does not
/// analyze or is keyed on another entity, a caller that does not own the TVIEW, or
/// a rebuild that is refused or fails.
pub(crate) fn create_or_replace(
    name: &str,
    query: &str,
    options: &serde_json::Value,
) -> TViewResult<&'static str> {
    let Name { schema, entity } = resolve::parse(name)?;
    let options = parse_options(options)?;
    let declares_time = matches!(options.time_refresh, Some(TimeRefresh::External { .. }));
    super::lock_entity(&entity)?;
    let tv_name = format!("tv_{entity}");

    let declared = options.resolve()?;
    let registered = resolve::registered_schema(&entity)?;

    let Some(meta) = TviewMeta::load_by_entity(&entity)? else {
        let schema = match schema {
            Some(schema) => schema,
            None => create::current_schema()?,
        };
        create_new(&entity, &schema, query, &declared)?;
        return Ok("created");
    };

    // An entity names one TVIEW in the whole database: unqualified, the name is
    // that TVIEW wherever it lives; qualified, the schema must be its own.
    let schema = match (schema, registered) {
        (None, Some(registered)) => registered,
        (Some(schema), Some(registered)) if schema == registered => schema,
        (Some(schema), registered) => {
            let registered = registered.unwrap_or_default();
            return Err(invalid(
                "tview",
                format!(
                    "TVIEW {entity} is registered in schema {registered}, not {schema}: an \
                     entity names one TVIEW in the whole database"
                ),
            ));
        }
        (None, None) => create::current_schema()?,
    };
    crate::owner::require_owner(meta.tview_oid, &tv_name)?;

    let qualified_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qualified_view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
    crate::utils::spi::run_ddl(&format!(
        "LOCK TABLE {qualified_tv}, {qualified_view} IN ACCESS SHARE MODE"
    ))?;

    let (normalized_sql, normalized) = create::normalize_definition(&entity, query)?;
    check_key(&entity, &normalized)?;
    let comparison = compare_definition(&entity, meta.view_oid, &normalized_sql)?;
    let current = Declared {
        storage: rebuild::current_storage(&entity)?,
        group_keys: create::stored_group_keys(&entity)?,
        declarations: Declarations::of(&meta),
        typename: stored_typename(&entity)?,
    };
    // Options other than the definition's own (storage, declarations, type name)
    // change in place.
    let alter = |current: &Declared| -> TViewResult<()> {
        if declared.storage != current.storage {
            alter_storage(
                &qualified_tv,
                &tv_name,
                &schema,
                meta.tview_oid,
                current.storage,
                declared.storage,
            )?;
        }
        if declared.typename != current.typename {
            store_typename(&entity, declared.typename.as_deref())?;
        }
        Ok(())
    };

    if comparison.same_view && declared.group_keys == current.group_keys {
        let retyped = retype_drifted_columns(&entity, &meta, &qualified_tv)?;
        let same_declarations = declared.declarations.stored() == current.declarations;
        if declared.storage == current.storage
            && same_declarations
            && declared.typename == current.typename
        {
            return Ok(if retyped { "altered" } else { "unchanged" });
        }
        alter(&current)?;
        if !same_declarations {
            // The stored policies are re-checked by a re-registration, which also
            // brings the triggers in line: `error` still refuses a TVIEW with
            // tables no cascade reaches.
            declared.declarations.store(&entity)?;
            create::reregister_tview(&entity)?;
        }
        check_time_declared(&entity, declares_time)?;
        return Ok("altered");
    }
    declared.declarations.store(&entity)?;
    if comparison.same_columns
        && declared.group_keys == current.group_keys
        && same_table_key(meta.tview_oid, comparison.identity.as_deref())?
    {
        replace_in_place(
            &entity,
            &schema,
            &meta,
            &qualified_view,
            &normalized_sql,
            &comparison.base_tables,
        )?;
        retype_drifted_columns(&entity, &meta, &qualified_tv)?;
        alter(&current)?;
        check_time_declared(&entity, declares_time)?;
        return Ok("replaced");
    }

    rebuild(&entity, &schema, &meta, query, declared)?;
    check_time_declared(&entity, declares_time)?;
    Ok("rebuilt")
}

/// The GraphQL type name stored for `entity`; `None` for `PascalCase(entity)`.
fn stored_typename(entity: &str) -> TViewResult<Option<String>> {
    Spi::get_one_with_args::<String>(
        &format!(
            "SELECT graphql_typename FROM {} WHERE entity = $1",
            crate::utils::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Read the GraphQL type name", &e))
}

/// Store the GraphQL type name of `entity`'s TVIEW (`None`: `PascalCase(entity)`).
///
/// # Errors
/// Returns an error if the catalog cannot be written.
pub(crate) fn store_typename(entity: &str, typename: Option<&str>) -> TViewResult<()> {
    let _owner = crate::owner::AsOwner::of_extension()?;
    Spi::run_with_args(
        &format!(
            "UPDATE {} SET graphql_typename = $2 WHERE entity = $1",
            crate::utils::meta_table()
        ),
        &[
            crate::utils::spi::text(entity),
            crate::utils::spi::text(typename),
        ],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Store the GraphQL type name", &e))
}

/// Refuse `time_refresh` passed for a TVIEW whose definition, as registered,
/// reads no time: the declaration would never apply. A re-registration
/// keeps a stored one silently.
fn check_time_declared(entity: &str, declared: bool) -> TViewResult<()> {
    if !declared {
        return Ok(());
    }
    let dependent = Spi::get_one_with_args::<bool>(
        &format!(
            "SELECT time_dependent FROM {} WHERE entity = $1",
            crate::utils::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Read whether a TVIEW reads the time", &e))?;
    if dependent == Some(true) {
        Ok(())
    } else {
        Err(invalid(
            "time_refresh",
            format!(
                "tv_{entity} declares time_refresh, but its definition reads no time: remove it"
            ),
        ))
    }
}

/// Give each column of the TVIEW the type of its backing view's column where the
/// two differ (a TVIEW created before its columns kept the view's types stored
/// enums, domains and composites as `text` and dropped typmods). The convention
/// columns, whose fixed types are by design, are left alone. Each column is
/// converted with a cast; one that fails aborts the replace, naming the column.
/// Returns whether a column changed.
fn retype_drifted_columns(entity: &str, meta: &TviewMeta, qualified_tv: &str) -> TViewResult<bool> {
    let stored: std::collections::HashMap<String, String> =
        crate::utils::column_types(meta.tview_oid)?
            .into_iter()
            .collect();
    let pk = format!("pk_{entity}");
    let mut retyped = false;
    for (column, view_type) in crate::utils::column_types(meta.view_oid)? {
        if column == pk
            || column == "id"
            || column == "data"
            || stored.get(&column).is_none_or(|t| *t == view_type)
        {
            continue;
        }
        let qi = ident::quoted(&column);
        crate::utils::spi::run_ddl(&format!(
            "ALTER TABLE {qualified_tv} ALTER COLUMN {qi} TYPE {view_type} USING {qi}::{view_type}"
        ))
        .map_err(|e| {
            invalid(
                "query",
                format!("cannot convert column {column} of {qualified_tv} to {view_type}: {e}"),
            )
        })?;
        retyped = true;
    }
    Ok(retyped)
}

/// What [`create_only`] found.
pub(crate) enum Created {
    /// Created, with this many rows.
    Rows(u64),
    /// The TVIEW existed and `IF NOT EXISTS` was given: nothing was done.
    Skipped,
    /// The TVIEW exists.
    Exists(String),
}

/// Create `name` from `query` with `CREATE TABLE AS` semantics: an existing TVIEW
/// is not replaced. `pg_tviews_create()` and an intercepted `CREATE TABLE tv_* AS`
/// run this code, which is the create path of [`create_or_replace`].
///
/// # Errors
/// Returns an error for an invalid name, a definition that does not analyze or is
/// keyed on another entity, or a failed creation.
pub(crate) fn create_only(
    name: &str,
    query: &str,
    options: Options,
    if_not_exists: bool,
) -> TViewResult<Created> {
    let Name { schema, entity } = resolve::parse(name)?;
    super::lock_entity(&entity)?;
    if TviewMeta::load_by_entity(&entity)?.is_some() {
        return Ok(if if_not_exists {
            notice!("TVIEW tv_{entity} already exists, skipping");
            Created::Skipped
        } else {
            Created::Exists(format!("tv_{entity}"))
        });
    }
    let schema = match schema {
        Some(schema) => schema,
        None => create::current_schema()?,
    };
    create_new(&entity, &schema, query, &options.resolve()?).map(Created::Rows)
}

/// Create `entity`'s TVIEW in `schema` as `declared`.
fn create_new(entity: &str, schema: &str, query: &str, declared: &Declared) -> TViewResult<u64> {
    let (_, normalized) = create::normalize_definition(entity, query)?;
    check_key(entity, &normalized)?;
    let rows = create::create_tview_in(
        &format!("tv_{entity}"),
        query,
        schema,
        declared.group_keys.as_ref(),
        declared.storage,
        Some(declared.declarations.clone()),
    )?;
    if declared.typename.is_some() {
        store_typename(entity, declared.typename.as_deref())?;
    }
    check_time_declared(
        entity,
        matches!(
            declared.declarations.time_refresh,
            TimeRefresh::External { declared: true }
        ),
    )?;
    Ok(rows)
}

/// The TVIEW's name must match the key its definition produces.
fn check_key(entity: &str, normalized: &create::ViewColumns) -> TViewResult<()> {
    match normalized.entity.as_deref() {
        Some(keyed) if keyed == entity => Ok(()),
        Some(keyed) => Err(invalid(
            "tview",
            format!(
                "TVIEW tv_{entity} does not match its definition, which is keyed on pk_{keyed}"
            ),
        )),
        None => Err(invalid(
            "query",
            format!("the definition of tv_{entity} has no pk_{entity} column"),
        )),
    }
}

/// Whether the new definition's identity (`key`, ADR 0169) is the column the
/// table's primary key is on now.
fn same_table_key(table: pg_sys::Oid, key: Option<&str>) -> TViewResult<bool> {
    let current = crate::utils::spi::strings(
        "SELECT a.attname::text FROM pg_catalog.pg_index i \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid \
          AND a.attnum = ANY (i.indkey) \
         WHERE i.indrelid = $1 AND i.indisprimary",
        &[crate::utils::spi::oid(table)],
    )?;
    Ok(key.is_some_and(|key| current == [key]))
}

/// How a new definition compares with a TVIEW's backing view.
struct Comparison {
    /// Rendered by `pg_get_viewdef` like the backing view: layout, comments and
    /// keyword case ignored, name resolution seen.
    same_view: bool,
    /// Same column names and types, in the same order.
    same_columns: bool,
    /// Ordinary and partitioned tables the new definition reads, through views.
    base_tables: Vec<pg_sys::Oid>,
    /// The column that names the new definition's rows; `None` when it has none
    /// (creating it raises why).
    identity: Option<String>,
}

/// Compare `definition` with the backing view `view_oid` through a temporary view.
/// An invalid definition raises its error.
fn compare_definition(
    entity: &str,
    view_oid: pg_sys::Oid,
    definition: &str,
) -> TViewResult<Comparison> {
    crate::utils::spi::run_ddl(&format!(
        "CREATE TEMP VIEW pg_tviews_candidate AS {definition}"
    ))?;
    let (same_view, same_columns) = Spi::connect(|client| {
        client
            .select(
                "WITH candidate AS (SELECT 'pg_temp.pg_tviews_candidate'::pg_catalog.regclass AS c) \
                 SELECT pg_catalog.pg_get_viewdef(c) = pg_catalog.pg_get_viewdef($1), \
                        (SELECT pg_catalog.array_agg(a.attname::text || ' ' \
                                || pg_catalog.format_type(a.atttypid, a.atttypmod) ORDER BY a.attnum) \
                         FROM pg_catalog.pg_attribute a \
                         WHERE a.attrelid = c AND a.attnum > 0 AND NOT a.attisdropped) \
                      = (SELECT pg_catalog.array_agg(a.attname::text || ' ' \
                                || pg_catalog.format_type(a.atttypid, a.atttypmod) ORDER BY a.attnum) \
                         FROM pg_catalog.pg_attribute a \
                         WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped) \
                 FROM candidate",
                None,
                &[crate::utils::spi::oid(view_oid)],
            )?
            .first()
            .get_two::<bool, bool>()
    })
    .map_err(|e| crate::utils::spi::catalog_error("Compare TVIEW definitions", &e))?;
    let base_tables = crate::utils::spi::oids(
        &format!(
            "{} SELECT DISTINCT r.relid FROM reads r WHERE r.relkind IN ('r', 'p')",
            crate::catalog::reads::view_reads_cte(
                "SELECT $1::pg_catalog.regclass::pg_catalog.oid, \
                        $1::pg_catalog.regclass::pg_catalog.oid, 0"
            )
        ),
        &[crate::utils::spi::text("pg_temp.pg_tviews_candidate")],
    )?;
    let candidate = crate::utils::spi::oids(
        "SELECT 'pg_temp.pg_tviews_candidate'::pg_catalog.regclass::pg_catalog.oid",
        &[],
    )?;
    let identity = candidate
        .first()
        .and_then(|&oid| crate::lineage::view_identity(entity, oid).ok())
        .map(|identity| identity.name);
    crate::utils::spi::run_ddl("DROP VIEW pg_temp.pg_tviews_candidate")?;
    Ok(Comparison {
        same_view: same_view == Some(true),
        same_columns: same_columns == Some(true),
        base_tables,
        identity,
    })
}

/// Replace the backing view with `definition`, which has its columns, and bring
/// the TVIEW and the TVIEWs that read its view to it in place: each is
/// re-registered and its rows reconciled, as its owner, writing only rows that
/// change. The tables, their indexes, privileges and dependents stay.
fn replace_in_place(
    entity: &str,
    schema: &str,
    meta: &TviewMeta,
    qualified_view: &str,
    definition: &str,
    new_base_tables: &[pg_sys::Oid],
) -> TViewResult<()> {
    let dependents = dependents(entity, meta.view_oid, meta.tview_oid)?;

    // Writers lock a base table, then the TVIEW tables their flush writes: take
    // the same order. SHARE on every table read, before or after, holds writers
    // off until the transaction ends; EXCLUSIVE on the TVIEW tables lets readers
    // go on. Each as the table's owner: SHARE needs more than SELECT.
    let mut entities = vec![entity.to_string()];
    entities.extend(dependents.iter().map(|(dependent, _)| dependent.clone()));
    let tv_tables: Vec<pg_sys::Oid> = std::iter::once(meta.tview_oid)
        .chain(dependents.iter().map(|&(_, table)| table))
        .collect();
    let mut read_tables = crate::utils::spi::oids(
        &format!(
            "SELECT DISTINCT r.relid FROM {}.pg_tview_reads r \
             JOIN pg_catalog.pg_class c ON c.oid = r.relid AND c.relkind IN ('r', 'p') \
             WHERE r.entity = ANY ($1)",
            crate::utils::ext_schema()
        ),
        &[crate::utils::spi::text_array_of(&entities)],
    )?;
    read_tables.extend_from_slice(new_base_tables);
    read_tables.retain(|table| !tv_tables.contains(table));
    read_tables.sort_unstable_by_key(|table| table.to_u32());
    read_tables.dedup();
    for &table in &read_tables {
        lock_as_owner(table, "SHARE")?;
    }
    for &table in &tv_tables {
        lock_as_owner(table, "EXCLUSIVE")?;
    }

    super::in_extension_schema(|| {
        crate::utils::spi::run_ddl(&format!(
            "CREATE OR REPLACE VIEW {qualified_view} AS {definition}"
        ))
    })?;
    let reads = create::reregister_metadata(entity, schema, definition)?;
    crate::dependency::sync_entity_triggers(&reads, entity)?;
    {
        // The rows are recomputed as the TVIEW's owner, as every refresh is.
        let _owner = crate::owner::AsOwner::of_table(meta.tview_oid)?;
        reconcile(entity, meta)?;
    }

    for (dependent, table) in &dependents {
        let _owner = crate::owner::AsOwner::of_table(*table)?;
        create::reregister_tview(dependent)?;
        let meta =
            TviewMeta::load_by_entity(dependent)?.ok_or_else(|| TViewError::TviewNotFound {
                name: dependent.clone(),
            })?;
        reconcile(dependent, &meta)?;
    }
    Ok(())
}

/// The TVIEWs whose view reads `view_oid`, directly or through views, as
/// `(entity, table)`, each after the others it reads.
fn dependents(
    entity: &str,
    view_oid: pg_sys::Oid,
    table_oid: pg_sys::Oid,
) -> TViewResult<Vec<(String, pg_sys::Oid)>> {
    let (meta_table, reads) = (
        crate::utils::meta_table(),
        format!("{}.pg_tview_reads", crate::utils::ext_schema()),
    );
    // Its view or its table. A TVIEW reads everything the TVIEWs it reads do, and
    // their views: it reads more of the others' views and tables than any of them.
    Spi::connect(|client| {
        let mut dependents = Vec::new();
        for row in client.select(
            &format!(
                "SELECT m.entity::text, m.table_oid FROM {meta_table} m \
                 JOIN pg_catalog.pg_class c ON c.oid = m.table_oid \
                 WHERE m.entity <> $1 \
                   AND EXISTS (SELECT 1 FROM {reads} r \
                               WHERE r.entity = m.entity AND r.relid IN ($2, $3)) \
                 ORDER BY (SELECT count(*) FROM {reads} r JOIN {meta_table} o \
                           ON r.relid IN (o.view_oid, o.table_oid) AND o.entity <> r.entity \
                           WHERE r.entity = m.entity), m.entity"
            ),
            None,
            &[
                crate::utils::spi::text(entity),
                crate::utils::spi::oid(view_oid),
                crate::utils::spi::oid(table_oid),
            ],
        )? {
            if let (Some(dependent), Some(table)) =
                (row.get::<String>(1)?, row.get::<pg_sys::Oid>(2)?)
            {
                dependents.push((dependent, table));
            }
        }
        Ok::<_, spi::Error>(dependents)
    })
    .map_err(|e| crate::utils::spi::catalog_error("Find the TVIEWs reading a replaced one", &e))
}

/// `LOCK TABLE` in `mode`, as the table's owner.
fn lock_as_owner(table: pg_sys::Oid, mode: &str) -> TViewResult<()> {
    let qualified = crate::utils::qualified_relname_from_oid(table)?;
    let _owner = crate::owner::AsOwner::of_table(table)?;
    crate::utils::spi::run_ddl(&format!("LOCK TABLE {qualified} IN {mode} MODE"))
}

/// Bring the rows of a TVIEW's table to those of its backing view with three
/// statements that touch only rows that change, journaling each change. Rows
/// that leave go first, so a unique index holds throughout. Returns the
/// `pk_<entity>` of every row deleted, updated or inserted.
pub(crate) fn reconcile(entity: &str, meta: &TviewMeta) -> TViewResult<Vec<String>> {
    use crate::queue::affected::{Change, record};
    let _pin = crate::owner::RenderPin::new();
    // Every row is computed: writers of anything they read wait, and are waited for.
    crate::concurrency::reads::lock_whole_read_set(meta)?;

    // A key names one row: a UNION view returning several for one is refused.
    crate::refresh::refuse_duplicate_keys(meta, "true", &[])?;
    let qualified_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qualified_view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;

    let columns = crate::utils::get_view_columns_by_oid(meta.view_oid)?;
    let keys = crate::utils::spi::strings(
        "SELECT a.attname::text FROM pg_catalog.pg_index i \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid \
          AND a.attnum = ANY (i.indkey) \
         WHERE i.indrelid = $1 AND i.indisprimary ORDER BY a.attnum",
        &[crate::utils::spi::oid(meta.tview_oid)],
    )?;
    let prefixed = |columns: &[&String], prefix: &str| -> Vec<String> {
        columns
            .iter()
            .map(|c| format!("{prefix}{}", ident::quoted(c)))
            .collect()
    };
    let list = |columns: &[&String], prefix: &str| prefixed(columns, prefix).join(", ");
    let key_columns: Vec<&String> = keys.iter().collect();
    let value_columns: Vec<&String> = columns.iter().filter(|c| !keys.contains(c)).collect();
    let all_columns: Vec<&String> = columns.iter().collect();
    let same_key = format!(
        "({}) = ({})",
        list(&key_columns, "t."),
        list(&key_columns, "v.")
    );
    let pk = ident::quoted(&format!("pk_{entity}"));

    let deleted = Spi::connect_mut(|client| {
        let mut rows = Vec::new();
        for row in client.update(
            &format!(
                "DELETE FROM {qualified_tv} t \
                 WHERE NOT EXISTS (SELECT 1 FROM {qualified_view} v WHERE {same_key}) \
                 RETURNING t.{pk}::text, pg_catalog.to_jsonb(t.*)->>'id'"
            ),
            None,
            &[],
        )? {
            if let Some(key) = row.get::<String>(1)? {
                rows.push((key, row.get::<String>(2)?));
            }
        }
        Ok::<_, spi::Error>(rows)
    })
    .map_err(|e| {
        crate::utils::spi::catalog_error("Delete the rows the new definition drops", &e)
    })?;
    let mut changed: Vec<String> = deleted.iter().map(|(key, _)| key.clone()).collect();
    for (key, id) in deleted {
        record(entity, key, Change::Deleted(id));
    }
    if !value_columns.is_empty() {
        // A stored column can have another type than the view's (an unmapped user type
        // is stored as text): compare against the value the UPDATE would assign.
        let stored: std::collections::HashMap<String, String> =
            crate::utils::column_types(meta.tview_oid)?
                .into_iter()
                .collect();
        let stored_types: Vec<String> = value_columns
            .iter()
            .filter_map(|c| stored.get(c.as_str()).cloned())
            .collect();
        if stored_types.len() != value_columns.len() {
            return Err(TViewError::CatalogError {
                operation: format!("Compare the rows of {qualified_tv} with its view"),
                pg_error: "a view column is missing from the TVIEW's table".to_string(),
            });
        }
        let fresh: Vec<String> = prefixed(&value_columns, "v.")
            .into_iter()
            .zip(&stored_types)
            .map(|(v, ty)| format!("{v}::{ty}"))
            .collect();
        let set = value_columns
            .iter()
            .map(|c| format!("{0} = v.{0}", ident::quoted(c)))
            .collect::<Vec<_>>()
            .join(", ");
        for key in crate::utils::spi::strings(
            &format!(
                "UPDATE {qualified_tv} t SET {set}, updated_at = pg_catalog.now() \
                 FROM {qualified_view} v \
                 WHERE {same_key} AND {} \
                 RETURNING t.{pk}::text",
                crate::refresh::rows_differ(&prefixed(&value_columns, "t."), &fresh)
            ),
            &[],
        )? {
            changed.push(key.clone());
            record(entity, key, Change::Updated);
        }
    }
    for key in crate::utils::spi::strings(
        &format!(
            "INSERT INTO {qualified_tv} ({columns}) \
             SELECT {columns} FROM {qualified_view} v \
             WHERE NOT EXISTS (SELECT 1 FROM {qualified_tv} t WHERE {same_key}) \
             RETURNING {pk}::text",
            columns = list(&all_columns, "")
        ),
        &[],
    )? {
        changed.push(key.clone());
        record(entity, key, Change::Inserted);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::parse_options;

    #[test]
    fn test_parse_options_rejects_unknown_and_mistyped() {
        let unknown = parse_options(&serde_json::json!({"colour": "red"})).unwrap_err();
        assert!(unknown.to_string().contains("unknown option \"colour\""));
        assert!(parse_options(&serde_json::json!({"fillfactor": "85"})).is_err());
        assert!(parse_options(&serde_json::json!({"fillfactor": 5})).is_err());
        assert!(parse_options(&serde_json::json!({"logged": 1})).is_err());
        assert!(parse_options(&serde_json::json!({"group_keys": {}})).is_err());
        assert!(parse_options(&serde_json::json!([])).is_err());
    }

    #[test]
    fn test_parse_options_group_keys_null_is_plain() {
        let options = parse_options(&serde_json::json!({"group_keys": null})).unwrap();
        assert_eq!(options.group_keys, None);
    }

    #[test]
    fn test_parse_options_typename() {
        let options = parse_options(&serde_json::json!({"typename": "BlogPost"})).unwrap();
        assert_eq!(options.typename.as_deref(), Some("BlogPost"));
        assert_eq!(
            parse_options(&serde_json::json!({"typename": null}))
                .unwrap()
                .typename,
            None
        );
        assert!(parse_options(&serde_json::json!({"typename": "blog-post"})).is_err());
        assert!(parse_options(&serde_json::json!({"typename": "9Lives"})).is_err());
    }
}

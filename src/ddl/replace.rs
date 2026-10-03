//! `pg_tviews_create_or_replace()`: create a TVIEW, or bring an existing one to a
//! definition and storage options with the smallest change (issue #134, ADR 0136
//! Decision 5).
//!
//! - **created**: the TVIEW did not exist.
//! - **unchanged**: the definition and the options passed match what exists.
//! - **altered**: only `logged`, `fillfactor` or `data_gin_index` differ; changed in
//!   place, rows kept.
//! - **replaced**: the definition differs but produces the same columns, and
//!   `group_keys` is the same; the backing view is replaced and the rows reconciled
//!   in place, touching only rows that change.
//! - **rebuilt**: the columns or `group_keys` differ; the TVIEW is dropped and
//!   created again, with its owner, privileges, comment, GraphQL type name and user
//!   indexes carried over. Refused when something depends on it or it has what a
//!   rebuild cannot carry.
//!
//! The DDL runs as the caller; replacing an existing TVIEW requires owning it.

use super::aggregate::GroupKeys;
use super::create::{self, Storage};
use crate::catalog::TviewMeta;
use crate::error::{TViewError, TViewResult};
use crate::schema::TViewSchema;
use crate::utils::quote_identifier;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Options of `pg_tviews_create_or_replace()`. `None`: not passed, so the default
/// on create and the current value on an existing TVIEW.
#[derive(Debug, Default)]
pub(crate) struct Options {
    logged: Option<bool>,
    fillfactor: Option<i32>,
    data_gin_index: Option<bool>,
    group_keys: GroupKeysOption,
}

/// The `group_keys` option.
#[derive(Debug, Default, PartialEq, Eq)]
enum GroupKeysOption {
    /// Not passed.
    #[default]
    Omitted,
    /// `null`: a plain TVIEW.
    Plain,
    /// An aggregate TVIEW with these group keys.
    Aggregate(GroupKeys),
}

impl Options {
    /// Options of a `CREATE [UNLOGGED] TABLE tv_* [WITH (fillfactor = n)] AS`.
    pub(crate) fn storage(logged: Option<bool>, fillfactor: Option<i32>) -> Self {
        Self {
            logged,
            fillfactor,
            ..Self::default()
        }
    }

    /// Options of `pg_tviews_create_aggregate()`.
    pub(crate) fn aggregate(group_keys: GroupKeys) -> Self {
        Self {
            group_keys: GroupKeysOption::Aggregate(group_keys),
            ..Self::default()
        }
    }
}

impl GroupKeysOption {
    /// The group keys asked for, `current` when omitted.
    fn or(self, current: Option<GroupKeys>) -> Option<GroupKeys> {
        match self {
            Self::Omitted => current,
            Self::Plain => None,
            Self::Aggregate(keys) => Some(keys),
        }
    }
}

fn invalid(parameter: &str, reason: impl Into<String>) -> TViewError {
    TViewError::InvalidInput {
        parameter: parameter.to_string(),
        reason: reason.into(),
    }
}

/// Parse the `options` object: an unknown key or a value of the wrong type is an
/// error.
///
/// # Errors
/// Returns an error naming the offending key.
pub(crate) fn parse_options(value: &serde_json::Value) -> TViewResult<Options> {
    let serde_json::Value::Object(map) = value else {
        return Err(invalid("options", "must be a JSON object"));
    };
    let mut options = Options::default();
    for (key, value) in map {
        match key.as_str() {
            "logged" => {
                options.logged = Some(
                    value
                        .as_bool()
                        .ok_or_else(|| invalid(key, "must be a boolean"))?,
                );
            }
            "data_gin_index" => {
                options.data_gin_index = Some(
                    value
                        .as_bool()
                        .ok_or_else(|| invalid(key, "must be a boolean"))?,
                );
            }
            "fillfactor" => {
                let fillfactor = value
                    .as_i64()
                    .ok_or_else(|| invalid(key, "must be an integer"))?;
                if !(10..=100).contains(&fillfactor) {
                    return Err(invalid(key, "must be between 10 and 100"));
                }
                options.fillfactor = i32::try_from(fillfactor).ok();
            }
            "group_keys" => {
                options.group_keys = match value {
                    serde_json::Value::Null => GroupKeysOption::Plain,
                    serde_json::Value::Object(keys) if !keys.is_empty() => {
                        GroupKeysOption::Aggregate(
                            serde_json::from_value::<GroupKeys>(value.clone()).map_err(|_| {
                                invalid(key, "must map source table names to column names")
                            })?,
                        )
                    }
                    _ => {
                        return Err(invalid(
                            key,
                            "must be null or an object mapping each source table to its \
                             group key column, e.g. {\"tb_order\": \"fk_user\"}",
                        ));
                    }
                };
            }
            other => {
                return Err(invalid(
                    "options",
                    format!(
                        "unknown option \"{other}\" (known: logged, fillfactor, \
                         data_gin_index, group_keys)"
                    ),
                ));
            }
        }
    }
    Ok(options)
}

/// Split a TVIEW name, `tv_<entity>`, `<entity>` or `schema.tv_<entity>`, into
/// its schema (if named) and entity. A part is taken as written, as the names of
/// earlier releases were; double-quote it (`""` for a quote) to include a dot.
///
/// # Errors
/// Returns an error if the name does not parse, or the entity is not a valid
/// identifier.
pub(crate) fn parse_name(name: &str) -> TViewResult<(Option<String>, String)> {
    let mut parts = split_identifiers(name)
        .ok_or_else(|| invalid("tview_name", format!("{name} is not a valid TVIEW name")))?;
    let table = parts.pop().unwrap_or_default();
    let schema = match parts.as_slice() {
        [] => None,
        [schema] => Some(schema.clone()),
        _ => {
            return Err(invalid(
                "tview_name",
                format!("{name} has too many dotted parts: use schema.tv_<entity>"),
            ));
        }
    };
    crate::validation::validate_sql_identifier(&table, "tview_name")?;
    let entity = table.strip_prefix("tv_").unwrap_or(&table);
    Ok((schema, entity.to_string()))
}

/// The dot-separated parts of `name`, double-quoted parts unquoted, or `None` if a
/// part is empty or a quote is not closed.
fn split_identifiers(name: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut chars = name.chars().peekable();
    loop {
        let mut part = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next()? {
                    '"' if chars.peek() == Some(&'"') => {
                        chars.next();
                        part.push('"');
                    }
                    '"' => break,
                    c => part.push(c),
                }
            }
            if !matches!(chars.peek(), None | Some('.')) {
                return None;
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == '.' {
                    break;
                }
                part.push(c);
                chars.next();
            }
        }
        if part.is_empty() {
            return None;
        }
        parts.push(part);
        if chars.next().is_none() {
            return Some(parts);
        }
    }
}

/// Schema of a registered entity: that of its `tv_*` table, or of its view when
/// the table is gone.
///
/// # Errors
/// Returns an error if the catalog query fails.
pub(crate) fn registered_schema(entity: &str) -> TViewResult<Option<String>> {
    Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT n.nspname::text FROM {} m \
                     JOIN pg_catalog.pg_class c \
                       ON c.oid = COALESCE((SELECT t.oid FROM pg_catalog.pg_class t \
                                           WHERE t.oid = m.table_oid), m.view_oid) \
                     JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                     WHERE m.entity = $1",
                    crate::utils::meta_table()
                ),
                None,
                &[text(entity)],
            )?
            .first()
            .get_one::<String>()
    })
    .map_err(|e| catalog("Find the schema of a TVIEW", &e))
}

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
    let (schema, entity) = parse_name(name)?;
    let options = parse_options(options)?;
    super::lock_entity(&entity)?;
    let schema = match schema {
        Some(schema) => schema,
        None => create::current_schema()?,
    };
    let tv_name = format!("tv_{entity}");

    let Some(meta) = TviewMeta::load_by_entity(&entity)? else {
        create_new(&entity, &schema, query, options)?;
        return Ok("created");
    };

    // An entity is unique across the database.
    if let Some(registered) = registered_schema(&entity)?
        && registered != schema
    {
        return Err(invalid(
            "tview_name",
            format!(
                "TVIEW {entity} is registered in schema {registered}, not {schema}: an entity \
                 names one TVIEW in the whole database"
            ),
        ));
    }
    crate::owner::require_owner(meta.tview_oid, &tv_name)?;

    let qualified_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qualified_view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
    run(&format!(
        "LOCK TABLE {qualified_tv}, {qualified_view} IN ACCESS SHARE MODE"
    ))?;

    let (normalized_sql, normalized) = create::normalize_definition(&entity, query)?;
    check_key(&entity, &normalized)?;
    let comparison = compare_definition(&entity, meta.view_oid, &normalized_sql)?;

    let current = current_storage(&entity)?;
    let desired = Storage {
        logged: options.logged.unwrap_or(current.logged),
        fillfactor: options.fillfactor.unwrap_or(current.fillfactor),
        data_gin_index: options.data_gin_index.unwrap_or(current.data_gin_index),
    };
    let current_keys = create::stored_group_keys(&entity)?;
    let desired_keys = options.group_keys.or(current_keys.clone());

    if comparison.same_view && desired_keys == current_keys {
        let retyped = retype_drifted_columns(&entity, &meta, &qualified_tv)?;
        if desired == current {
            return Ok(if retyped { "altered" } else { "unchanged" });
        }
        alter_storage(
            &qualified_tv,
            &tv_name,
            &schema,
            meta.tview_oid,
            current,
            desired,
        )?;
        return Ok("altered");
    }
    if comparison.same_columns
        && desired_keys == current_keys
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
        if desired != current {
            alter_storage(
                &qualified_tv,
                &tv_name,
                &schema,
                meta.tview_oid,
                current,
                desired,
            )?;
        }
        return Ok("replaced");
    }

    rebuild(
        &entity,
        &schema,
        &meta,
        query,
        desired,
        desired_keys.as_ref(),
    )?;
    Ok("rebuilt")
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
            || meta.fk_columns.contains(&column)
            || stored.get(&column).is_none_or(|t| *t == view_type)
        {
            continue;
        }
        let qi = quote_identifier(&column);
        run(&format!(
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
/// is not replaced (issue #134). `pg_tviews_create()`, `pg_tviews_create_aggregate()`
/// and an intercepted `CREATE TABLE tv_* AS` run this code, which is the create
/// path of [`create_or_replace`].
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
    let (schema, entity) = parse_name(name)?;
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
    create_new(&entity, &schema, query, options).map(Created::Rows)
}

/// Create `entity`'s TVIEW in `schema`: storage options default to the settings.
fn create_new(entity: &str, schema: &str, query: &str, options: Options) -> TViewResult<u64> {
    let (_, normalized) = create::normalize_definition(entity, query)?;
    check_key(entity, &normalized)?;
    let defaults = Storage::from_settings();
    let storage = Storage {
        logged: options.logged.unwrap_or(defaults.logged),
        fillfactor: options.fillfactor.unwrap_or(defaults.fillfactor),
        data_gin_index: options.data_gin_index.unwrap_or(defaults.data_gin_index),
    };
    create::create_tview_in(
        &format!("tv_{entity}"),
        query,
        schema,
        options.group_keys.or(None).as_ref(),
        storage,
        None,
    )
}

/// The TVIEW's name must match the key its definition produces.
fn check_key(entity: &str, normalized: &TViewSchema) -> TViewResult<()> {
    match normalized.entity_name.as_deref() {
        Some(keyed) if keyed == entity => Ok(()),
        Some(keyed) => Err(invalid(
            "tview_name",
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
    let current = strings(
        "SELECT a.attname::text FROM pg_catalog.pg_index i \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid \
          AND a.attnum = ANY (i.indkey) \
         WHERE i.indrelid = $1 AND i.indisprimary",
        &[oid(table)],
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
    run(&format!(
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
                &[oid(view_oid)],
            )?
            .first()
            .get_two::<bool, bool>()
    })
    .map_err(|e| catalog("Compare TVIEW definitions", &e))?;
    let base_tables = oids(
        &format!(
            "{BASE_TABLES} FROM reads r JOIN pg_catalog.pg_class c ON c.oid = r.relid \
             AND c.relkind IN ('r', 'p')"
        ),
        &[text("pg_temp.pg_tviews_candidate")],
    )?;
    let candidate = oids(
        "SELECT 'pg_temp.pg_tviews_candidate'::pg_catalog.regclass::pg_catalog.oid",
        &[],
    )?;
    let identity = candidate
        .first()
        .and_then(|&oid| crate::lineage::view_identity(entity, oid).ok())
        .map(|identity| identity.name);
    run("DROP VIEW pg_temp.pg_tviews_candidate")?;
    Ok(Comparison {
        same_view: same_view == Some(true),
        same_columns: same_columns == Some(true),
        base_tables,
        identity,
    })
}

/// The relations a view (`$1`, a regclass name) reads, followed through views, as
/// `reads(relid)`; the caller completes the `SELECT … FROM reads`.
const BASE_TABLES: &str = "\
    WITH RECURSIVE reads(relid) AS ( \
        SELECT $1::pg_catalog.regclass::oid \
      UNION \
        SELECT d.refobjid FROM reads r \
        JOIN pg_catalog.pg_class v ON v.oid = r.relid AND v.relkind = 'v' \
        JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid \
        JOIN pg_catalog.pg_depend d \
          ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND d.objid = w.oid \
         AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
         AND d.refobjid <> v.oid) \
    SELECT c.oid";

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
    let dependents = dependents(entity, meta.view_oid)?;

    // Writers lock a base table, then the TVIEW tables their flush writes: take
    // the same order. SHARE on every table read, before or after, holds writers
    // off until the transaction ends; EXCLUSIVE on the TVIEW tables lets readers
    // go on. Each as the table's owner: SHARE needs more than SELECT.
    let mut entities = vec![entity.to_string()];
    entities.extend(dependents.iter().map(|(dependent, _)| dependent.clone()));
    let tv_tables: Vec<pg_sys::Oid> = std::iter::once(meta.tview_oid)
        .chain(dependents.iter().map(|&(_, table)| table))
        .collect();
    let mut read_tables = oids(
        &format!(
            "SELECT DISTINCT r.relid FROM {}.pg_tview_reads r \
             JOIN pg_catalog.pg_class c ON c.oid = r.relid AND c.relkind IN ('r', 'p') \
             WHERE r.entity = ANY ($1)",
            crate::utils::ext_schema()
        ),
        &[texts(&entities)],
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

    run(&format!(
        "CREATE OR REPLACE VIEW {qualified_view} AS {definition}"
    ))?;
    let reads = create::reregister_metadata(entity, schema, definition)?;
    crate::dependency::sync_entity_triggers(&reads, entity)?;
    reconcile(entity, meta)?;

    for (dependent, table) in &dependents {
        let _owner = crate::owner::AsOwner::of_table(*table)?;
        create::reregister_tview(dependent)?;
        let meta =
            TviewMeta::load_by_entity(dependent)?.ok_or_else(|| TViewError::MetadataNotFound {
                entity: dependent.clone(),
            })?;
        reconcile(dependent, &meta)?;
    }
    Ok(())
}

/// The TVIEWs whose view reads `view_oid`, directly or through views, as
/// `(entity, table)`, each after the others it reads.
fn dependents(entity: &str, view_oid: pg_sys::Oid) -> TViewResult<Vec<(String, pg_sys::Oid)>> {
    let (meta_table, reads) = (
        crate::utils::meta_table(),
        format!("{}.pg_tview_reads", crate::utils::ext_schema()),
    );
    // A TVIEW reads everything the TVIEWs it reads do, and their views: it reads
    // more of the others' views than any of them.
    Spi::connect(|client| {
        let mut dependents = Vec::new();
        for row in client.select(
            &format!(
                "SELECT m.entity::text, m.table_oid FROM {meta_table} m \
                 JOIN pg_catalog.pg_class c ON c.oid = m.table_oid \
                 WHERE m.entity <> $1 \
                   AND EXISTS (SELECT 1 FROM {reads} r WHERE r.entity = m.entity AND r.relid = $2) \
                 ORDER BY (SELECT count(*) FROM {reads} r JOIN {meta_table} o \
                           ON o.view_oid = r.relid AND o.entity <> r.entity \
                           WHERE r.entity = m.entity), m.entity"
            ),
            None,
            &[text(entity), oid(view_oid)],
        )? {
            if let (Some(dependent), Some(table)) =
                (row.get::<String>(1)?, row.get::<pg_sys::Oid>(2)?)
            {
                dependents.push((dependent, table));
            }
        }
        Ok::<_, spi::Error>(dependents)
    })
    .map_err(|e| catalog("Find the TVIEWs reading a replaced one", &e))
}

/// `LOCK TABLE` in `mode`, as the table's owner.
fn lock_as_owner(table: pg_sys::Oid, mode: &str) -> TViewResult<()> {
    let qualified = crate::utils::qualified_relname_from_oid(table)?;
    let _owner = crate::owner::AsOwner::of_table(table)?;
    run(&format!("LOCK TABLE {qualified} IN {mode} MODE"))
}

/// Bring the rows of a TVIEW's table to those of its backing view with three
/// statements that touch only rows that change, journaling each change. Rows
/// that leave go first, so a unique index holds throughout. Returns the
/// `pk_<entity>` of every row deleted, updated or inserted.
pub(crate) fn reconcile(entity: &str, meta: &TviewMeta) -> TViewResult<Vec<String>> {
    use crate::queue::affected::{Change, record};

    let qualified_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qualified_view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;

    let columns = crate::utils::get_view_columns_by_oid(meta.view_oid)?;
    let keys = strings(
        "SELECT a.attname::text FROM pg_catalog.pg_index i \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid \
          AND a.attnum = ANY (i.indkey) \
         WHERE i.indrelid = $1 AND i.indisprimary ORDER BY a.attnum",
        &[oid(meta.tview_oid)],
    )?;
    let prefixed = |columns: &[&String], prefix: &str| -> Vec<String> {
        columns
            .iter()
            .map(|c| format!("{prefix}{}", quote_identifier(c)))
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
    let pk = quote_identifier(&format!("pk_{entity}"));

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
    .map_err(|e| catalog("Delete the rows the new definition drops", &e))?;
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
            .map(|c| format!("{0} = v.{0}", quote_identifier(c)))
            .collect::<Vec<_>>()
            .join(", ");
        for key in strings(
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
    for key in strings(
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

/// The table's actual storage, as `tviews.registry` reports it.
fn current_storage(entity: &str) -> TViewResult<Storage> {
    let (logged, fillfactor, data_gin_index) = Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT (options->>'logged')::boolean, (options->>'fillfactor')::integer, \
                            (options->>'data_gin_index')::boolean \
                     FROM {}.registry WHERE entity = $1",
                    crate::utils::ext_schema()
                ),
                None,
                &[text(entity)],
            )?
            .first()
            .get_three::<bool, i32, bool>()
    })
    .map_err(|e| catalog("Read TVIEW storage", &e))?;
    Ok(Storage {
        logged: logged.unwrap_or(true),
        fillfactor: fillfactor.unwrap_or(100),
        data_gin_index: data_gin_index.unwrap_or(false),
    })
}

/// Change the storage of a TVIEW's table in place, keeping its rows.
fn alter_storage(
    qualified_tv: &str,
    tv_name: &str,
    schema: &str,
    table: pg_sys::Oid,
    current: Storage,
    desired: Storage,
) -> TViewResult<()> {
    if desired.logged != current.logged {
        let persistence = if desired.logged { "LOGGED" } else { "UNLOGGED" };
        run(&format!("ALTER TABLE {qualified_tv} SET {persistence}"))?;
    }
    if desired.fillfactor != current.fillfactor {
        if desired.fillfactor == 100 {
            run(&format!("ALTER TABLE {qualified_tv} RESET (fillfactor)"))?;
        } else {
            run(&format!(
                "ALTER TABLE {qualified_tv} SET (fillfactor = {})",
                desired.fillfactor
            ))?;
        }
    }
    if desired.data_gin_index && !current.data_gin_index {
        run(&create::index_ddl(
            schema,
            tv_name,
            "data_gin",
            "USING GIN ",
            &["data"],
        ))?;
    } else if !desired.data_gin_index && current.data_gin_index {
        for index in strings(
            "SELECT i.indexrelid::pg_catalog.regclass::text FROM pg_catalog.pg_index i \
             JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid \
             JOIN pg_catalog.pg_am am ON am.oid = ic.relam AND am.amname = 'gin' \
             JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid \
              AND a.attname = 'data' AND a.attnum = i.indkey[0] \
             WHERE i.indrelid = $1 AND i.indnatts = 1 AND i.indpred IS NULL",
            &[oid(table)],
        )? {
            run(&format!("DROP INDEX {index}"))?;
        }
    }
    Ok(())
}

/// Drop the TVIEW and create it again from `query`, carrying over what the table
/// had that the definition does not describe.
fn rebuild(
    entity: &str,
    schema: &str,
    meta: &TviewMeta,
    query: &str,
    storage: Storage,
    group_keys: Option<&GroupKeys>,
) -> TViewResult<()> {
    let tv_name = format!("tv_{entity}");
    let objects = [oid(meta.tview_oid), oid(meta.view_oid)];

    let refusals = strings(REBUILD_REFUSALS, &objects)?;
    if !refusals.is_empty() {
        return Err(invalid(
            "query",
            format!(
                "TVIEW {tv_name} must be rebuilt for this change, which would lose: {}. \
                 Remove them first, or keep the definition's columns and group_keys",
                refusals.join("; ")
            ),
        ));
    }

    // What the rebuild must put back, as statements computed before the drop.
    let restore = strings(RESTORE_STATEMENTS, &objects)?;
    let graphql_typename = Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT graphql_typename FROM {} WHERE entity = $1",
                    crate::utils::meta_table()
                ),
                None,
                &[text(entity)],
            )?
            .first()
            .get_one::<String>()
    })
    .map_err(|e| catalog("Read the GraphQL type name", &e))?;
    let user_indexes = user_indexes(entity, &tv_name, meta.tview_oid)?;

    super::drop::drop_tview(
        &format!("{}.{tv_name}", quote_identifier(schema)),
        false,
        false,
    )?;
    create::create_tview_in(
        &tv_name,
        query,
        schema,
        group_keys,
        storage,
        Some(meta.uncascaded_policy),
    )?;

    let (tv, view) = (
        format!(
            "{}.{}",
            quote_identifier(schema),
            quote_identifier(&tv_name)
        ),
        format!(
            "{}.{}",
            quote_identifier(schema),
            quote_identifier(&format!("v_{entity}"))
        ),
    );
    // Owners first; then the new objects' default privileges give way to the saved
    // ones; then comments.
    let (owners, others): (Vec<&String>, Vec<&String>) =
        restore.iter().partition(|s| s.starts_with("ALTER "));
    for statement in owners {
        run(statement)?;
    }
    for revoke in strings(
        "SELECT pg_catalog.format('REVOKE ALL ON %s FROM %s', c.oid::pg_catalog.regclass, \
             CASE WHEN a.grantee = 0 THEN 'PUBLIC' \
                  ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(a.grantee)) END) \
         FROM pg_catalog.pg_class c, pg_catalog.aclexplode(c.relacl) a \
         WHERE c.oid IN ($1::pg_catalog.regclass, $2::pg_catalog.regclass) \
           AND a.grantee <> c.relowner \
         GROUP BY c.oid, a.grantee",
        &[text(&tv), text(&view)],
    )? {
        run(&revoke)?;
    }
    for statement in others {
        run(statement)?;
    }
    if let Some(typename) = graphql_typename {
        let _owner = crate::owner::AsOwner::of_extension()?;
        Spi::run_with_args(
            &format!(
                "UPDATE {} SET graphql_typename = $2 WHERE entity = $1",
                crate::utils::meta_table()
            ),
            &[text(entity), text(&typename)],
        )
        .map_err(|e| catalog("Restore the GraphQL type name", &e))?;
    }
    for (index, definition) in user_indexes {
        // Re-run inside a block that names the index when it no longer applies.
        let wrapped = Spi::connect(|client| {
            client
                .select(
                    "SELECT pg_catalog.format('DO %L', pg_catalog.format(\
                         'BEGIN EXECUTE %L; EXCEPTION WHEN OTHERS THEN RAISE EXCEPTION \
                          USING MESSAGE = %L || SQLERRM, ERRCODE = SQLSTATE; END', $1, $2))",
                    None,
                    &[
                        text(&definition),
                        text(&format!(
                            "index {index} on {tv_name} cannot be re-created after the \
                             rebuild: "
                        )),
                    ],
                )?
                .first()
                .get_one::<String>()
        })
        .map_err(|e| catalog("Prepare a user index", &e))?
        .unwrap_or_default();
        run(&wrapped)?;
    }
    Ok(())
}

/// Why a TVIEW (`$1` its table, `$2` its backing view) cannot be rebuilt: objects
/// that depend on it, and table properties a rebuild would drop.
const REBUILD_REFUSALS: &str = "\
    SELECT DISTINCT pg_catalog.pg_describe_object(d.classid, d.objid, d.objsubid) \
           || ' depends on it' \
    FROM pg_catalog.pg_depend d \
    WHERE d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
      AND d.refobjid IN ($1, $2) AND d.deptype = 'n' \
      AND NOT (d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass \
               AND (SELECT ev_class FROM pg_catalog.pg_rewrite WHERE oid = d.objid) = $2) \
      AND NOT (d.classid = 'pg_catalog.pg_constraint'::pg_catalog.regclass \
               AND (SELECT conrelid FROM pg_catalog.pg_constraint WHERE oid = d.objid) = $1) \
  UNION ALL SELECT 'row level security is enabled' FROM pg_catalog.pg_class \
    WHERE oid = $1 AND (relrowsecurity OR relforcerowsecurity) \
  UNION ALL SELECT pg_catalog.format('policy %I', polname) FROM pg_catalog.pg_policy \
    WHERE polrelid = $1 \
  UNION ALL SELECT pg_catalog.format('trigger %I', tgname) FROM pg_catalog.pg_trigger \
    WHERE tgrelid = $1 AND NOT tgisinternal \
  UNION ALL SELECT pg_catalog.format('rule %I', rulename) FROM pg_catalog.pg_rewrite \
    WHERE ev_class = $1 \
  UNION ALL SELECT pg_catalog.format('membership in publication %I', p.pubname) \
    FROM pg_catalog.pg_publication_rel r JOIN pg_catalog.pg_publication p ON p.oid = r.prpubid \
    WHERE r.prrelid = $1 \
  UNION ALL SELECT 'a replica identity other than the default' FROM pg_catalog.pg_class \
    WHERE oid = $1 AND relreplident <> 'd' \
  UNION ALL SELECT pg_catalog.format('a statistics target on column %I', attname) \
    FROM pg_catalog.pg_attribute \
    WHERE attrelid = $1 AND attnum > 0 AND NOT attisdropped \
      AND COALESCE(attstattarget::integer, -1) >= 0 \
  UNION ALL SELECT pg_catalog.format('privileges on column %I', attname) \
    FROM pg_catalog.pg_attribute \
    WHERE attrelid = $1 AND attnum > 0 AND NOT attisdropped AND attacl IS NOT NULL \
  UNION ALL SELECT pg_catalog.format('statistics object %I', stxname) \
    FROM pg_catalog.pg_statistic_ext WHERE stxrelid = $1 \
  UNION ALL SELECT 'a security label' FROM pg_catalog.pg_seclabel \
    WHERE classoid = 'pg_catalog.pg_class'::pg_catalog.regclass AND objoid IN ($1, $2) \
  UNION ALL SELECT pg_catalog.format('constraint %I', conname) FROM pg_catalog.pg_constraint \
    WHERE conrelid = $1 AND contype NOT IN ('p', 'n') \
  UNION ALL SELECT pg_catalog.format('a comment on column %I', a.attname) \
    FROM pg_catalog.pg_description d JOIN pg_catalog.pg_attribute a \
      ON a.attrelid = d.objoid AND a.attnum = d.objsubid \
    WHERE d.classoid = 'pg_catalog.pg_class'::pg_catalog.regclass \
      AND d.objoid IN ($1, $2) AND d.objsubid > 0";

/// Statements that give the rebuilt table (`$1`) and view (`$2`) the owner,
/// privileges and comment they have now, in the order to run them.
const RESTORE_STATEMENTS: &str = "\
    WITH objects(kind, relid) AS (VALUES ('TABLE', $1), ('VIEW', $2)) \
    SELECT statement FROM ( \
        SELECT 1 AS step, pg_catalog.format('ALTER %s %s OWNER TO %I', o.kind, \
               c.oid::pg_catalog.regclass, pg_catalog.pg_get_userbyid(c.relowner)) AS statement \
        FROM objects o JOIN pg_catalog.pg_class c ON c.oid = o.relid \
        WHERE c.relowner <> (SELECT oid FROM pg_catalog.pg_roles WHERE rolname = CURRENT_USER) \
      UNION ALL \
        SELECT 2, pg_catalog.format('GRANT %s ON %s TO %s%s', a.privilege_type, \
               c.oid::pg_catalog.regclass, \
               CASE WHEN a.grantee = 0 THEN 'PUBLIC' \
                    ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(a.grantee)) END, \
               CASE WHEN a.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END) \
        FROM objects o JOIN pg_catalog.pg_class c ON c.oid = o.relid, \
             pg_catalog.aclexplode(c.relacl) a \
        WHERE a.grantee <> c.relowner \
      UNION ALL \
        SELECT 3, pg_catalog.format('COMMENT ON %s %s IS %L', o.kind, \
               c.oid::pg_catalog.regclass, d.description) \
        FROM objects o JOIN pg_catalog.pg_class c ON c.oid = o.relid \
        JOIN pg_catalog.pg_description d ON d.objoid = c.oid \
         AND d.classoid = 'pg_catalog.pg_class'::pg_catalog.regclass AND d.objsubid = 0 \
    ) s ORDER BY step, statement";

/// Indexes a user added to the TVIEW's table, as `(name, definition)`: every
/// index that backs no constraint and is not one `pg_tviews` creates for the
/// current definition.
fn user_indexes(
    entity: &str,
    tv_name: &str,
    table: pg_sys::Oid,
) -> TViewResult<Vec<(String, String)>> {
    let (definition, embeds) = Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT definition, aggregate_embeds FROM {} WHERE entity = $1",
                    crate::utils::meta_table()
                ),
                None,
                &[text(entity)],
            )?
            .first()
            .get_two::<String, pgrx::JsonB>()
    })
    .map_err(|e| catalog("Read the TVIEW definition", &e))?;
    let schema = crate::schema::inference::infer_schema(&definition.unwrap_or_default())?;
    let embed_columns: Vec<String> = embeds
        .and_then(|j| {
            serde_json::from_value::<std::collections::BTreeMap<String, String>>(j.0).ok()
        })
        .map(|m| m.into_values().collect())
        .unwrap_or_default();
    let managed = create::managed_index_names(tv_name, &schema, &embed_columns);

    Spi::connect(|client| {
        let mut indexes = Vec::new();
        for row in client.select(
            "SELECT ic.relname::text, pg_catalog.pg_get_indexdef(i.indexrelid) \
             FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid \
             WHERE i.indrelid = $1 \
               AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k \
                               WHERE k.conindid = i.indexrelid) \
             ORDER BY 1",
            None,
            &[oid(table)],
        )? {
            if let (Some(name), Some(definition)) = (row.get::<String>(1)?, row.get::<String>(2)?)
                && !managed.contains(&name)
            {
                indexes.push((name, definition));
            }
        }
        Ok::<_, spi::Error>(indexes)
    })
    .map_err(|e| catalog("List the TVIEW's indexes", &e))
}

fn run(sql: &str) -> TViewResult<()> {
    crate::utils::spi_run_ddl(sql).map_err(|e| TViewError::SpiError {
        query: sql.to_string(),
        error: e,
    })
}

/// The first column of every row of `query`.
fn strings(query: &str, args: &[DatumWithOid<'_>]) -> TViewResult<Vec<String>> {
    Spi::connect(|client| {
        let mut values = Vec::new();
        for row in client.select(query, None, args)? {
            if let Some(value) = row.get::<String>(1)? {
                values.push(value);
            }
        }
        Ok::<_, spi::Error>(values)
    })
    .map_err(|e| catalog("Read the TVIEW's catalog entries", &e))
}

fn oids(query: &str, args: &[DatumWithOid<'_>]) -> TViewResult<Vec<pg_sys::Oid>> {
    Spi::connect(|client| {
        let mut oids = Vec::new();
        for row in client.select(query, None, args)? {
            if let Some(oid) = row.get::<pg_sys::Oid>(1)? {
                oids.push(oid);
            }
        }
        Ok::<_, spi::Error>(oids)
    })
    .map_err(|e| catalog("Read the tables a TVIEW reads", &e))
}

fn texts(values: &[String]) -> DatumWithOid<'static> {
    // SAFETY: the datum copies the strings.
    unsafe {
        DatumWithOid::new(
            values.to_vec(),
            PgOid::BuiltIn(PgBuiltInOids::TEXTARRAYOID).value(),
        )
    }
}

fn text(value: &str) -> DatumWithOid<'_> {
    // SAFETY: the datum borrows `value` for the lifetime of the returned datum.
    unsafe { DatumWithOid::new(value, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }
}

fn oid(value: pg_sys::Oid) -> DatumWithOid<'static> {
    // SAFETY: the datum copies the OID.
    unsafe { DatumWithOid::new(value, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }
}

fn catalog(operation: &str, e: &spi::Error) -> TViewError {
    TViewError::CatalogError {
        operation: operation.to_string(),
        pg_error: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{GroupKeysOption, parse_name, parse_options};

    #[test]
    fn test_parse_name_forms() {
        assert_eq!(parse_name("tv_post").unwrap(), (None, "post".to_string()));
        assert_eq!(parse_name("post").unwrap(), (None, "post".to_string()));
        assert_eq!(
            parse_name("app.tv_post").unwrap(),
            (Some("app".to_string()), "post".to_string())
        );
        assert!(parse_name("app.tv post").is_err());
    }

    #[test]
    fn test_parse_name_quoting() {
        assert_eq!(
            parse_name("\"Odd.Schema\".tv_post").unwrap(),
            (Some("Odd.Schema".to_string()), "post".to_string())
        );
        assert_eq!(
            parse_name("\"a\"\"b\".\"tv_post\"").unwrap(),
            (Some("a\"b".to_string()), "post".to_string())
        );
        assert_eq!(
            parse_name("App.tv_Post").unwrap(),
            (Some("App".to_string()), "Post".to_string())
        );
        for bad in [
            "\"app.tv_post",
            "app..tv_post",
            "a.b.tv_post",
            "\"app\"x.tv_post",
            "",
        ] {
            assert!(parse_name(bad).is_err(), "{bad}");
        }
    }

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
        assert_eq!(options.group_keys, GroupKeysOption::Plain);
        let options = parse_options(&serde_json::json!({})).unwrap();
        assert_eq!(options.group_keys, GroupKeysOption::Omitted);
    }
}

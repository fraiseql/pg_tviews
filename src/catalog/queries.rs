//! The catalog's reads and writes for the DDL, the hooks, the maintenance API
//! and the reports: every statement naming `pg_tview_meta` outside the decoding
//! of [`super::TviewMeta`] lives here or beside it in the catalog module.

use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;

// --- Registration ------------------------------------------------------------

/// The declarations of a TVIEW as the catalog stores them: the uncascaded
/// policy, the tables and functions declared, the time refresh.
pub struct Declared<'a> {
    pub policy: &'a str,
    pub table_oids: Vec<Oid>,
    pub table_policies: Vec<String>,
    pub function_read_functions: Vec<String>,
    pub function_read_tables: Vec<Oid>,
    pub time_refresh: Option<&'a str>,
}

/// A TVIEW's row as registration derives it.
pub struct Registration<'a> {
    pub entity: &'a str,
    pub view_oid: Oid,
    pub table_oid: Oid,
    pub definition: &'a str,
    pub plan: serde_json::Value,
    pub group_keys: Option<serde_json::Value>,
    pub uncascaded_oids: Vec<Oid>,
    pub declared: Declared<'a>,
    pub identity_kind: &'a str,
    pub identity_column: &'a str,
    pub identity_type: Oid,
    pub time_dependent: bool,
}

/// Insert `row`, or with `replace` overwrite what registration derives of an
/// existing one (keeping `created_at`, `graphql_typename` and
/// `needs_reregister`), as the extension's owner.
///
/// # Errors
/// A [`TViewError::SpiError`] when the row cannot be written.
pub fn write_registration(row: Registration<'_>, replace: bool) -> TViewResult<()> {
    let on_conflict = if replace {
        "ON CONFLICT (entity) DO UPDATE SET \
            view_oid = EXCLUDED.view_oid, table_oid = EXCLUDED.table_oid, \
            definition = EXCLUDED.definition, plan = EXCLUDED.plan, \
            group_keys = EXCLUDED.group_keys, uncascaded_oids = EXCLUDED.uncascaded_oids, \
            identity = EXCLUDED.identity, time_refresh = EXCLUDED.time_refresh, \
            time_dependent = EXCLUDED.time_dependent"
    } else {
        "ON CONFLICT (entity) DO NOTHING"
    };
    // Every value is a parameter (Q22).
    let sql = format!(
        "INSERT INTO {} (
            entity, view_oid, table_oid, definition, plan, group_keys,
            uncascaded_oids, uncascaded_policy, identity,
            uncascaded_table_oids, uncascaded_table_policies,
            function_read_functions, function_read_tables, time_refresh, time_dependent
        ) VALUES (
            $1, $2::pg_catalog.oid::pg_catalog.regclass, $3::pg_catalog.oid::pg_catalog.regclass,
            $4, $5, $6, $7::pg_catalog.oid[]::pg_catalog.regclass[], $8,
            pg_catalog.jsonb_build_object('kind', $9::pg_catalog.text, 'columns',
                pg_catalog.jsonb_build_array(pg_catalog.jsonb_build_object(
                    'name', $10::pg_catalog.text,
                    'type', pg_catalog.format_type($11::pg_catalog.oid, NULL)))),
            $12::pg_catalog.oid[]::pg_catalog.regclass[], $13,
            $14, $15::pg_catalog.oid[]::pg_catalog.regclass[], $16, $17)
        {on_conflict}",
        super::meta_table()
    );
    let Registration {
        entity,
        view_oid,
        table_oid,
        definition,
        plan,
        group_keys,
        uncascaded_oids,
        declared,
        identity_kind,
        identity_column,
        identity_type,
        time_dependent,
    } = row;
    let args = [
        crate::utils::spi::text(entity),
        crate::utils::spi::oid(view_oid),
        crate::utils::spi::oid(table_oid),
        crate::utils::spi::text(definition),
        crate::utils::spi::jsonb(pgrx::JsonB(plan)),
        crate::utils::spi::jsonb(group_keys.map(pgrx::JsonB)),
        crate::utils::spi::oid_array(uncascaded_oids),
        crate::utils::spi::text(declared.policy),
        crate::utils::spi::text(identity_kind),
        crate::utils::spi::text(identity_column),
        crate::utils::spi::oid(identity_type),
        crate::utils::spi::oid_array(declared.table_oids),
        crate::utils::spi::text_array(declared.table_policies),
        crate::utils::spi::text_array(declared.function_read_functions),
        crate::utils::spi::oid_array(declared.function_read_tables),
        crate::utils::spi::text(declared.time_refresh),
        crate::utils::spi::boolean(time_dependent),
    ];
    let _owner = crate::owner::AsOwner::of_extension()?;
    Spi::run_with_args(&sql, &args).map_err(|e| TViewError::SpiError {
        query: sql,
        error: e.to_string(),
    })
}

/// Store `declared` for `entity`, as the extension's owner.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the row cannot be written.
pub fn store_declared(entity: &str, declared: Declared<'_>) -> TViewResult<()> {
    let _owner = crate::owner::AsOwner::of_extension()?;
    let args = [
        crate::utils::spi::text(entity),
        crate::utils::spi::text(declared.policy),
        crate::utils::spi::oid_array(declared.table_oids),
        crate::utils::spi::text_array(declared.table_policies),
        crate::utils::spi::text_array(declared.function_read_functions),
        crate::utils::spi::oid_array(declared.function_read_tables),
        crate::utils::spi::text(declared.time_refresh),
    ];
    Spi::run_with_args(
        &format!(
            "UPDATE {} SET uncascaded_policy = $2, \
                 uncascaded_table_oids = $3::pg_catalog.oid[]::pg_catalog.regclass[], \
                 uncascaded_table_policies = $4, function_read_functions = $5, \
                 function_read_tables = $6::pg_catalog.oid[]::pg_catalog.regclass[], \
                 time_refresh = $7 \
             WHERE entity = $1",
            super::meta_table()
        ),
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: "Store the uncascaded policies".to_string(),
        pg_error: e.to_string(),
    })
}

/// Rename the group key column `old` of `relname` to `new` in `entity`'s stored
/// group keys, as the extension's owner.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the row cannot be written.
pub fn rename_group_key(entity: &str, relname: &str, old: &str, new: &str) -> TViewResult<()> {
    let _owner = crate::owner::AsOwner::of_extension()?;
    Spi::run_with_args(
        &format!(
            "UPDATE {} SET group_keys = jsonb_set(group_keys, ARRAY[$2], to_jsonb($4)) \
             WHERE entity = $1 AND group_keys->>$2 = $3",
            super::meta_table()
        ),
        &[
            crate::utils::spi::text(entity),
            crate::utils::spi::text(relname),
            crate::utils::spi::text(old),
            crate::utils::spi::text(new),
        ],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Rename a group key column", &e))
}

/// Delete `entity`'s row, as the extension's owner.
///
/// # Errors
/// Returns the statement's error.
pub fn delete(entity: &str) -> TViewResult<()> {
    let args = [crate::utils::spi::text(entity)];
    let sql = format!("DELETE FROM {} WHERE entity = $1", super::meta_table());
    // The catalog is written as the extension's owner.
    let _owner = crate::owner::AsOwner::of_extension()?;
    crate::utils::spi::run(&sql, &args)
}

// --- Drops -------------------------------------------------------------------

/// Whether the current `sql_drop` event dropped `entity`'s view or table.
///
/// # Errors
/// Returns the query's error (outside a `sql_drop` event trigger, among others).
pub fn dropped_by_event(entity: &str) -> TViewResult<Option<bool>> {
    crate::utils::spi::one::<bool>(
        &format!(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger_dropped_objects() d \
             JOIN {} m ON d.objid IN (m.view_oid, m.table_oid) \
             WHERE m.entity = $1 AND d.objsubid = 0 \
               AND d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass)",
            super::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
}

/// Whether `entity`'s table is left, and whether the current role owns it.
///
/// # Errors
/// A [`TViewError::SpiError`] when the catalog cannot be read.
pub fn table_left_and_owned(entity: &str) -> TViewResult<(Option<bool>, Option<bool>)> {
    Spi::get_two_with_args::<bool, bool>(
        &format!(
            "SELECT t.oid IS NOT NULL, \
                    COALESCE(pg_catalog.pg_has_role(t.relowner, 'USAGE'), false) \
             FROM {} m LEFT JOIN pg_catalog.pg_class t ON t.oid = m.table_oid \
             WHERE m.entity = $1",
            super::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
    .map_err(|e| TViewError::SpiError {
        query: "table of a dropped TVIEW".to_string(),
        error: e.to_string(),
    })
}

/// `entity`'s backing view, while it exists.
///
/// # Errors
/// Returns the query's error.
pub fn backing_view(entity: &str) -> TViewResult<Option<Oid>> {
    crate::utils::spi::one::<Oid>(
        &format!(
            "SELECT (SELECT v.oid FROM {} m JOIN pg_catalog.pg_class v ON v.oid = m.view_oid \
             WHERE m.entity = $1)",
            super::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
}

/// The backing views of every registered TVIEW in the extension's schema, read
/// before `DROP EXTENSION pg_tviews` removes the catalog. A view outside
/// it (the layout of releases before 0.1.0-beta.25) is the application's and stays.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn extension_backing_views() -> TViewResult<Vec<Oid>> {
    // An install older than the library (0.1.0 kept its catalog elsewhere, and
    // its backing views were the application's `v_*` views): nothing to drop.
    let current = crate::utils::spi::one::<bool>(
        "SELECT pg_catalog.to_regclass($1) IS NOT NULL",
        &[crate::utils::spi::text(super::meta_table().as_str())],
    )?;
    if current != Some(true) {
        return Ok(Vec::new());
    }
    Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT v.oid FROM {} m JOIN pg_catalog.pg_class v ON v.oid = m.view_oid \
                     WHERE v.relnamespace = '{}'::pg_catalog.regnamespace",
                    super::meta_table(),
                    crate::utils::ext_schema()
                ),
                None,
                &[],
            )?
            .map(|row| row.get::<Oid>(1))
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|e| TViewError::SpiError {
        query: "backing views of the registered TVIEWs".to_string(),
        error: e.to_string(),
    })
}

/// The view `schema.name` when no TVIEW is registered with it and nothing
/// depends on it.
///
/// # Errors
/// Returns the query's error.
pub fn leftover_view(schema: &str, name: &str) -> TViewResult<Option<Oid>> {
    crate::utils::spi::one::<Oid>(
        &format!(
            "SELECT (SELECT c.oid FROM pg_catalog.pg_class c \
                     JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind = 'v' \
                       AND NOT EXISTS (SELECT 1 FROM {} m WHERE m.view_oid = c.oid) \
                       AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
                                       WHERE d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
                                         AND d.refobjid = c.oid AND d.deptype = 'n'))",
            super::meta_table()
        ),
        &[
            crate::utils::spi::text(schema),
            crate::utils::spi::text(name),
        ],
    )
}

// --- Backing view owners and grants ------------------------------------------

/// For each TVIEW (the one whose table is `$1`, every one when `$1` is NULL), the
/// statements that give its backing view the `SELECT` grants of its table, as
/// `(view, statement)`. The view's owner is left out: it has every privilege.
const SELECT_GRANT_CHANGES: &str = "\
    WITH tview AS ( \
        SELECT m.table_oid::pg_catalog.oid AS tab, v.oid AS view, v.relowner AS owner, \
               pg_catalog.format('%I.%I', n.nspname, v.relname) AS name \
        FROM {meta} m JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid \
        JOIN pg_catalog.pg_namespace n ON n.oid = v.relnamespace \
        WHERE $1 IS NULL OR m.table_oid::pg_catalog.oid = $1 \
    ), wanted AS ( \
        SELECT t.view, t.name, a.grantee FROM tview t JOIN pg_catalog.pg_class c ON c.oid = t.tab, \
               pg_catalog.aclexplode(c.relacl) a \
        WHERE a.privilege_type = 'SELECT' AND a.grantee <> t.owner \
    ), held AS ( \
        SELECT t.view, t.name, a.grantee FROM tview t JOIN pg_catalog.pg_class c ON c.oid = t.view, \
               pg_catalog.aclexplode(c.relacl) a \
        WHERE a.privilege_type = 'SELECT' AND a.grantee <> t.owner \
    ), changes AS ( \
        SELECT view, name, grantee, true AS adds FROM (TABLE wanted EXCEPT TABLE held) g \
        UNION ALL \
        SELECT view, name, grantee, false FROM (TABLE held EXCEPT TABLE wanted) r \
    ) \
    SELECT view, pg_catalog.format(CASE WHEN adds THEN 'GRANT SELECT ON %s TO %s' \
                                        ELSE 'REVOKE SELECT ON %s FROM %s CASCADE' END, \
               name, \
               pg_catalog.string_agg(CASE WHEN grantee = 0 THEN 'PUBLIC' \
                   ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(grantee)) END, \
                   ', ' ORDER BY grantee)) \
    FROM changes GROUP BY view, name, adds ORDER BY view, adds";

/// The `GRANT`/`REVOKE` statements that give the backing view of the TVIEW whose
/// table is `table` (of every TVIEW when `None`) the `SELECT` grants of that
/// table, as `(view, statement)`. Read-write: a fresh snapshot, which sees the
/// statement just run.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
pub fn grant_changes(table: Option<Oid>) -> TViewResult<Vec<(Option<Oid>, Option<String>)>> {
    let args = [crate::utils::spi::oid(table)];
    Spi::connect_mut(|client| {
        client
            .update(
                &SELECT_GRANT_CHANGES.replace("{meta}", &super::meta_table()),
                None,
                &args,
            )?
            .map(|row| Ok((row.get::<Oid>(1)?, row.get::<String>(2)?)))
            .collect::<Result<Vec<_>, spi::Error>>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Compare backing view grants with their tables'".to_string(),
        pg_error: e.to_string(),
    })
}

/// The `ALTER VIEW … OWNER TO` statements that give each backing view whose
/// owner is not its table's that owner (only the TVIEW whose table is `table`,
/// unless `None`), as `(new owner, statement)`. Read-write: a fresh snapshot.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
pub fn owner_changes(table: Option<Oid>) -> TViewResult<Vec<(Option<Oid>, Option<String>)>> {
    let args = [crate::utils::spi::oid(table)];
    Spi::connect_mut(|client| {
        client
            .update(
                &format!(
                    "SELECT t.relowner, pg_catalog.format('ALTER VIEW %I.%I OWNER TO %I', \
                            n.nspname, v.relname, pg_catalog.pg_get_userbyid(t.relowner)) \
                     FROM {} m \
                     JOIN pg_catalog.pg_class t ON t.oid = m.table_oid::pg_catalog.oid \
                     JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid \
                     JOIN pg_catalog.pg_namespace n ON n.oid = v.relnamespace \
                     WHERE v.relowner <> t.relowner AND ($1 IS NULL OR t.oid = $1) \
                     ORDER BY v.oid",
                    super::meta_table()
                ),
                None,
                &args,
            )?
            .map(|row| Ok((row.get::<Oid>(1)?, row.get::<String>(2)?)))
            .collect::<Result<Vec<_>, spi::Error>>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Compare backing view owners with their tables'".to_string(),
        pg_error: e.to_string(),
    })
}

// --- Readers -----------------------------------------------------------------

/// The TVIEWs whose definitions read the current time and whose table the
/// caller owns (or may change as a member of its owner's role), by entity.
///
/// # Errors
/// Returns the query's error.
pub fn owned_time_dependent() -> TViewResult<Vec<String>> {
    crate::utils::spi::strings(
        &format!(
            "SELECT m.entity::text FROM {} m JOIN pg_catalog.pg_class c ON c.oid = m.table_oid \
             WHERE m.time_dependent AND pg_catalog.pg_has_role(c.relowner, 'USAGE') \
             ORDER BY m.entity",
            super::meta_table()
        ),
        &[],
    )
}

/// TVIEWs whose backing view's own rule depends on column `column` of `relid`
/// (the column-level `pg_depend` entries of its rewrite rule): the definitions
/// that name the column, as `(entity, schema, view)`. A view the TVIEW reads
/// follows a rename by itself.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
pub fn reading_column(relid: Oid, column: &str) -> TViewResult<Vec<(String, String, Oid)>> {
    let query = format!(
        "{} SELECT DISTINCT m.entity, n.nspname::text AS schema, m.view_oid::oid AS view_oid \
         FROM reads r \
         JOIN {} m ON m.view_oid::oid = r.root \
         JOIN pg_catalog.pg_class t ON t.oid = m.table_oid \
         JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = r.relid AND a.attnum = r.attnum \
         WHERE r.depth = 0 AND r.relid = $1 AND a.attname = $2 \
         ORDER BY m.entity",
        super::reads::view_reads_cte(&format!(
            "SELECT m.view_oid::pg_catalog.oid, m.view_oid::pg_catalog.oid, 0 FROM {} m",
            super::meta_table()
        )),
        super::meta_table()
    );
    Spi::connect(|client| {
        let args = [
            crate::utils::spi::oid(relid),
            crate::utils::spi::text(column),
        ];
        let mut out = Vec::new();
        for row in client.select(&query, None, &args)? {
            let entity: Option<String> = row["entity"].value()?;
            let schema: Option<String> = row["schema"].value()?;
            let view_oid: Option<Oid> = row["view_oid"].value()?;
            if let (Some(e), Some(s), Some(v)) = (entity, schema, view_oid) {
                out.push((e, s, v));
            }
        }
        Ok::<_, spi::Error>(out)
    })
    .map_err(|e| crate::utils::spi::catalog_error("Find TVIEWs reading the renamed column", &e))
}

/// The TVIEWs other than `entity` whose view reads its view `view_oid` or its
/// table `table_oid`, directly or through views, as `(entity, table)`, each after
/// the others it reads.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
pub fn readers_of(entity: &str, view_oid: Oid, table_oid: Oid) -> TViewResult<Vec<(String, Oid)>> {
    let (meta_table, reads) = (
        super::meta_table(),
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
            if let (Some(dependent), Some(table)) = (row.get::<String>(1)?, row.get::<Oid>(2)?) {
                dependents.push((dependent, table));
            }
        }
        Ok::<_, spi::Error>(dependents)
    })
    .map_err(|e| crate::utils::spi::catalog_error("Find the TVIEWs reading a replaced one", &e))
}

/// The TVIEWs reading any of `columns` of `table` (or reading it with columns
/// unknown), by entity: what a new generation expression of those columns
/// changes.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
#[cfg(not(feature = "pg16"))]
pub fn column_readers_of(table: Oid, columns: &[String]) -> TViewResult<Vec<String>> {
    entities(
        &format!(
            "SELECT DISTINCT m.entity::text FROM {} m, \
                 pg_catalog.jsonb_array_elements(m.plan->'tables') t \
             WHERE (t->>'relid')::pg_catalog.oid = $1 \
               AND (pg_catalog.jsonb_array_length(COALESCE(t->'attnums', '[]')) = 0 \
                    OR EXISTS (SELECT 1 FROM pg_catalog.jsonb_array_elements_text(t->'attnums') n \
                               JOIN pg_catalog.pg_attribute a \
                                 ON a.attrelid = $1 AND a.attnum = n::pg_catalog.int2 \
                               WHERE a.attname::pg_catalog.text = ANY ($2))) \
             ORDER BY 1",
            super::meta_table()
        ),
        &[
            crate::utils::spi::oid(table),
            crate::utils::spi::text_array(columns.to_vec()),
        ],
        "Find the TVIEWs reading a column",
    )
}

/// The TVIEWs whose values show the enum `enum_type`, by entity: they read a
/// column of it (directly, through a domain over it or as an array of it), or
/// their backing view names it. What renaming one of its labels changes.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
pub fn type_readers_of(enum_type: Oid) -> TViewResult<Vec<String>> {
    entities(
        &format!(
            "WITH RECURSIVE types(oid) AS ( \
                 SELECT $1::pg_catalog.oid \
                 UNION SELECT t.oid FROM pg_catalog.pg_type t, types \
                       WHERE t.typbasetype = types.oid OR t.typelem = types.oid) \
             SELECT DISTINCT m.entity::text FROM {} m \
             WHERE EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
                           JOIN pg_catalog.pg_rewrite r ON r.oid = d.objid \
                           WHERE d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass \
                             AND r.ev_class = m.view_oid::pg_catalog.oid \
                             AND d.refclassid = 'pg_catalog.pg_type'::pg_catalog.regclass \
                             AND d.refobjid IN (SELECT oid FROM types)) \
                OR EXISTS (SELECT 1 FROM pg_catalog.jsonb_array_elements(m.plan->'tables') t \
                           JOIN pg_catalog.pg_attribute a \
                             ON a.attrelid = (t->>'relid')::pg_catalog.oid \
                            AND a.attnum > 0 AND NOT a.attisdropped \
                           WHERE a.atttypid IN (SELECT oid FROM types)) \
             ORDER BY 1",
            super::meta_table()
        ),
        &[crate::utils::spi::oid(enum_type)],
        "Find the TVIEWs showing an enum",
    )
}

/// The entities `sql` returns, as `operation` names the failure.
fn entities(
    sql: &str,
    args: &[pgrx::datum::DatumWithOid<'_>],
    operation: &str,
) -> TViewResult<Vec<String>> {
    crate::utils::spi::strings(sql, args)
        .map_err(|e| crate::utils::spi::catalog_error(operation, &e))
}

/// The TVIEWs whose plan reads `table`, by entity, whatever triggers they have
/// on it.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
pub fn table_readers_of(table: Oid) -> TViewResult<Vec<String>> {
    entities(
        &format!(
            "SELECT DISTINCT m.entity::text FROM {} m, \
                 pg_catalog.jsonb_array_elements(m.plan->'tables') t \
             WHERE (t->>'relid')::pg_catalog.oid = $1 ORDER BY 1",
            super::meta_table()
        ),
        &[crate::utils::spi::oid(table)],
        "Find the TVIEWs reading a table",
    )
}

/// The TVIEWs reading the materialized view `matview` under the `full_refresh`
/// policy (their own for it, or the TVIEW's), by entity.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
pub fn full_refresh_readers_of(matview: Oid) -> TViewResult<Vec<String>> {
    let args = [crate::utils::spi::oid(matview)];
    Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT entity::text FROM {} \
                     WHERE $1::pg_catalog.oid::pg_catalog.regclass = ANY (uncascaded_oids) \
                       AND COALESCE(uncascaded_table_policies[pg_catalog.array_position( \
                               uncascaded_table_oids, $1::pg_catalog.oid::pg_catalog.regclass)], \
                           uncascaded_policy) = 'full_refresh' \
                     ORDER BY entity",
                    super::meta_table()
                ),
                None,
                &args,
            )?
            .map(|row| row.get::<String>(1))
            .collect::<Result<Vec<_>, _>>()
            .map(|entities| entities.into_iter().flatten().collect())
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Find the TVIEWs reading a refreshed materialized view".to_string(),
        pg_error: e.to_string(),
    })
}

// --- Relations ---------------------------------------------------------------

/// The catalog table's OID, once it exists: read from the system caches, without
/// a query (the kept plans' cache, which every catalog read goes through, asks
/// for it).
#[must_use]
pub fn meta_table_oid() -> Option<Oid> {
    let schema = std::ffi::CString::new(crate::utils::ext_schema()).ok()?;
    // SAFETY: catalog lookups by name inside a transaction; the names are
    // NUL-terminated.
    unsafe {
        let schema = pg_sys::get_namespace_oid(schema.as_ptr(), true);
        let table = pg_sys::get_relname_relid(c"pg_tview_meta".as_ptr(), schema);
        (schema != pg_sys::InvalidOid && table != pg_sys::InvalidOid).then_some(table)
    }
}

/// Table `table`, read inside a function, as `(relname, relkind, entity)`: the
/// entity when it is a TVIEW's table.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
pub fn function_read_relation(
    table: Oid,
) -> TViewResult<(Option<String>, Option<String>, Option<String>)> {
    Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT c.relname::text, c.relkind::text, \
                            (SELECT m.entity::text FROM {} m WHERE m.table_oid::pg_catalog.oid = c.oid) \
                     FROM pg_catalog.pg_class c WHERE c.oid = $1",
                    super::meta_table()
                ),
                None,
                &[crate::utils::spi::oid(table)],
            )?
            .first()
            .get_three::<String, String, String>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Read a table a function reads".to_string(),
        pg_error: e.to_string(),
    })
}

/// The type of column `column` of the backing view of the TVIEW whose table is
/// `table`; `None` when there is no such column.
///
/// # Errors
/// Returns the query's error.
pub fn view_column_type(table: Oid, column: &str) -> TViewResult<Option<Oid>> {
    crate::utils::spi::one::<Oid>(
        &format!(
            "SELECT a.atttypid FROM {} m JOIN pg_catalog.pg_attribute a \
             ON a.attrelid = m.view_oid::pg_catalog.oid \
             WHERE m.table_oid::pg_catalog.oid = $1 AND a.attname = $2 AND NOT a.attisdropped",
            super::meta_table()
        ),
        &[
            crate::utils::spi::oid(table),
            crate::utils::spi::text(column),
        ],
    )
}

/// A TVIEW's table, as [`tables`] lists it.
pub struct Table {
    pub entity: String,
    pub schema: String,
    pub name: String,
    pub unlogged: bool,
    pub oid: Oid,
}

/// The table of every registered TVIEW (or only `entity`'s), ordered by entity.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
pub fn tables(entity: Option<&str>) -> TViewResult<Vec<Table>> {
    let query = format!(
        "SELECT m.entity, n.nspname::text AS schema, t.relname::text AS tbl, \
                t.relpersistence = 'u' AS unlogged, \
                m.table_oid::oid AS table_oid \
         FROM {} m \
         JOIN pg_class t ON t.oid = m.table_oid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         WHERE $1::text IS NULL OR m.entity = $1 \
         ORDER BY m.entity",
        super::meta_table()
    );
    Spi::connect(|client| {
        let args = [crate::utils::spi::text(entity)];
        let mut out = Vec::new();
        for row in client.select(&query, None, &args)? {
            out.push(Table {
                entity: row["entity"].value()?.unwrap_or_default(),
                schema: row["schema"].value()?.unwrap_or_default(),
                name: row["tbl"].value()?.unwrap_or_default(),
                unlogged: row["unlogged"].value()?.unwrap_or(false),
                oid: row["table_oid"].value()?.unwrap_or(pg_sys::InvalidOid),
            });
        }
        Ok::<_, spi::Error>(out)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Load TVIEW relations".to_string(),
        pg_error: e.to_string(),
    })
}

/// The GraphQL type name ([`super::typename`]) and quoted, schema-qualified
/// table of each of `entities`, as `(entity, typename, table)`.
///
/// # Errors
/// A [`TViewError::CatalogError`] when the catalog cannot be read.
pub fn typenames_and_tables(entities: Vec<String>) -> TViewResult<Vec<(String, String, String)>> {
    Spi::connect(|client| {
        let args = [crate::utils::spi::text_array(entities)];
        let mut out = Vec::new();
        for row in client.select(
            &format!(
                "SELECT m.entity, m.graphql_typename, \
                        quote_ident(n.nspname) || '.' || quote_ident(c.relname) AS tbl \
                 FROM {} m \
                 JOIN pg_class c ON c.oid = m.table_oid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE m.entity = ANY($1)",
                super::meta_table()
            ),
            None,
            &args,
        )? {
            let entity: String = row["entity"].value()?.unwrap_or_default();
            let typename: Option<String> = row["graphql_typename"].value()?;
            let table: String = row["tbl"].value()?.unwrap_or_default();
            let typename = super::typename(&entity, typename);
            out.push((entity, typename, table));
        }
        Ok::<_, spi::Error>(out)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Load TVIEWs for the report".to_string(),
        pg_error: e.to_string(),
    })
}

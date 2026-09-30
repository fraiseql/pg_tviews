//! `pg_tviews_create_or_replace()`: create a TVIEW, or bring an existing one to a
//! definition and storage options with the smallest change (issue #134, ADR 0136
//! Decision 5).
//!
//! - **created**: the TVIEW did not exist.
//! - **unchanged**: the definition and the options passed match what exists.
//! - **altered**: only `logged`, `fillfactor` or `data_gin_index` differ; changed in
//!   place, rows kept.
//! - **rebuilt**: the definition or `group_keys` differ; the TVIEW is dropped and
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
/// its schema (if named) and entity.
///
/// # Errors
/// Returns an error if a part is not a valid identifier.
pub(crate) fn parse_name(name: &str) -> TViewResult<(Option<String>, String)> {
    let (schema, table) = match name.split_once('.') {
        Some((schema, table)) => (Some(schema), table),
        None => (None, name),
    };
    if let Some(schema) = schema {
        crate::validation::validate_sql_identifier(schema, "schema")?;
    }
    crate::validation::validate_sql_identifier(table, "tview_name")?;
    let entity = table.strip_prefix("tv_").unwrap_or(table);
    Ok((schema.map(str::to_string), entity.to_string()))
}

/// Schema of the `tv_*` table of a registered entity.
///
/// # Errors
/// Returns an error if the catalog query fails.
pub(crate) fn registered_schema(entity: &str) -> TViewResult<Option<String>> {
    Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT n.nspname::text FROM {} m \
                     JOIN pg_catalog.pg_class c ON c.oid = m.table_oid \
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
        let (_, normalized) = create::normalize_definition(&entity, query)?;
        check_key(&entity, &normalized)?;
        let defaults = Storage::from_settings();
        let storage = Storage {
            logged: options.logged.unwrap_or(defaults.logged),
            fillfactor: options.fillfactor.unwrap_or(defaults.fillfactor),
            data_gin_index: options.data_gin_index.unwrap_or(defaults.data_gin_index),
        };
        create::create_tview_in(
            &tv_name,
            query,
            &schema,
            options.group_keys.or(None).as_ref(),
            storage,
        )?;
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
    let same_query = defines_view(meta.view_oid, &normalized_sql)?;

    let current = current_storage(meta.tview_oid)?;
    let desired = Storage {
        logged: options.logged.unwrap_or(current.logged),
        fillfactor: options.fillfactor.unwrap_or(current.fillfactor),
        data_gin_index: options.data_gin_index.unwrap_or(current.data_gin_index),
    };
    let current_keys = create::stored_group_keys(&entity)?;
    let desired_keys = options.group_keys.or(current_keys.clone());

    if same_query && desired_keys == current_keys {
        if desired == current {
            return Ok("unchanged");
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

/// Whether `definition` defines the same view as `view_oid`: both rendered by
/// `pg_get_viewdef`, which ignores layout, comments and keyword case and sees name
/// resolution. An invalid definition raises its error.
fn defines_view(view_oid: pg_sys::Oid, definition: &str) -> TViewResult<bool> {
    run(&format!(
        "CREATE TEMP VIEW pg_tviews_candidate AS {definition}"
    ))?;
    let same = Spi::connect(|client| {
        client
            .select(
                "SELECT pg_catalog.pg_get_viewdef('pg_temp.pg_tviews_candidate'::pg_catalog.regclass) \
                      = pg_catalog.pg_get_viewdef($1)",
                None,
                &[oid(view_oid)],
            )?
            .first()
            .get_one::<bool>()
    })
    .map_err(|e| catalog("Compare TVIEW definitions", &e))?;
    run("DROP VIEW pg_temp.pg_tviews_candidate")?;
    Ok(same == Some(true))
}

/// The table's actual storage, read from the catalogs as `tviews.registry` does.
fn current_storage(table: pg_sys::Oid) -> TViewResult<Storage> {
    let (logged, fillfactor, data_gin_index) = Spi::connect(|client| {
        client
            .select(
                "SELECT c.relpersistence = 'p', \
                        COALESCE((SELECT o.option_value::integer \
                                  FROM pg_catalog.pg_options_to_table(c.reloptions) o \
                                  WHERE o.option_name = 'fillfactor'), 100), \
                        EXISTS (SELECT 1 FROM pg_catalog.pg_index i \
                                JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid \
                                JOIN pg_catalog.pg_am am ON am.oid = ic.relam \
                                 AND am.amname = 'gin' \
                                JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid \
                                 AND a.attname = 'data' AND a.attnum = i.indkey[0] \
                                WHERE i.indrelid = c.oid AND i.indnatts = 1 \
                                  AND i.indpred IS NULL) \
                 FROM pg_catalog.pg_class c WHERE c.oid = $1",
                None,
                &[oid(table)],
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

    super::drop::drop_tview(&format!("{schema}.{tv_name}"), false, false)?;
    create::create_tview_in(&tv_name, query, schema, group_keys, storage)?;

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
    SELECT pg_catalog.pg_describe_object(d.classid, d.objid, d.objsubid) || ' depends on it' \
    FROM pg_catalog.pg_depend d \
    WHERE d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
      AND d.refobjid IN ($1, $2) AND d.deptype = 'n' \
      AND NOT (d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass \
               AND (SELECT ev_class FROM pg_catalog.pg_rewrite WHERE oid = d.objid) = $2) \
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
    WHERE classoid = 'pg_catalog.pg_class'::pg_catalog.regclass AND objoid IN ($1, $2)";

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

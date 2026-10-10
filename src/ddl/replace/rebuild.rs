//! Storage changes in place, and the rebuild of a TVIEW whose columns change.

use super::{Spi, invalid, pg_sys, spi};
use crate::catalog::TviewMeta;
use crate::ddl::aggregate::GroupKeys;
use crate::ddl::create::{self, Storage};
use crate::ddl::uncascaded::Declarations;
use crate::error::{TViewError, TViewResult};
use crate::utils::quote_identifier;

/// The table's actual storage, as `tviews.registry` reports it.
pub(super) fn current_storage(entity: &str) -> TViewResult<Storage> {
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
                &[crate::utils::spi::text(entity)],
            )?
            .first()
            .get_three::<bool, i32, bool>()
    })
    .map_err(|e| crate::utils::spi::catalog_error("Read TVIEW storage", &e))?;
    Ok(Storage {
        logged: logged.unwrap_or(true),
        fillfactor: fillfactor.unwrap_or(100),
        data_gin_index: data_gin_index.unwrap_or(false),
    })
}

/// Change the storage of a TVIEW's table in place, keeping its rows.
pub(super) fn alter_storage(
    qualified_tv: &str,
    tv_name: &str,
    schema: &str,
    table: pg_sys::Oid,
    current: Storage,
    desired: Storage,
) -> TViewResult<()> {
    if desired.logged != current.logged {
        let persistence = if desired.logged { "LOGGED" } else { "UNLOGGED" };
        crate::utils::spi::run_ddl(&format!("ALTER TABLE {qualified_tv} SET {persistence}"))?;
    }
    if desired.fillfactor != current.fillfactor {
        if desired.fillfactor == 100 {
            crate::utils::spi::run_ddl(&format!("ALTER TABLE {qualified_tv} RESET (fillfactor)"))?;
        } else {
            crate::utils::spi::run_ddl(&format!(
                "ALTER TABLE {qualified_tv} SET (fillfactor = {})",
                desired.fillfactor
            ))?;
        }
    }
    if desired.data_gin_index && !current.data_gin_index {
        let created = create::ManagedIndex::data_gin(tv_name, "data").create(schema, tv_name)?;
        crate::catalog::indexes::record(table, &created.into_iter().collect::<Vec<_>>())?;
    } else if !desired.data_gin_index && current.data_gin_index {
        // pg_tviews' GIN index on data, under whatever name it was renamed to.
        let gin = crate::utils::spi::strings(
            "SELECT ic.relname::pg_catalog.text FROM pg_catalog.pg_index i \
             JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid \
             JOIN pg_catalog.pg_am am ON am.oid = ic.relam AND am.amname = 'gin' \
             JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid \
              AND a.attname = 'data' AND a.attnum = i.indkey[0] \
             WHERE i.indrelid = $1 AND i.indnatts = 1 AND i.indpred IS NULL \
               AND ic.relname = ANY ($2)",
            &[
                crate::utils::spi::oid(table),
                crate::utils::spi::text_array_of(&crate::catalog::indexes::recorded(table)?),
            ],
        )?;
        for index in &gin {
            crate::utils::spi::run_ddl(&format!(
                "DROP INDEX {}.{}",
                quote_identifier(schema),
                quote_identifier(index)
            ))?;
        }
        crate::catalog::indexes::forget(table, &gin)?;
    }
    Ok(())
}

/// Drop the TVIEW and create it again from `query`, carrying over what the table
/// had that the definition does not describe.
pub(super) fn rebuild(
    entity: &str,
    schema: &str,
    meta: &TviewMeta,
    query: &str,
    storage: Storage,
    group_keys: Option<&GroupKeys>,
    declarations: Declarations,
) -> TViewResult<()> {
    let tv_name = format!("tv_{entity}");
    let objects = [
        crate::utils::spi::oid(meta.tview_oid),
        crate::utils::spi::oid(meta.view_oid),
    ];

    let refusals = crate::utils::spi::strings(REBUILD_REFUSALS, &objects)?;
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
    let restore = crate::utils::spi::strings(RESTORE_STATEMENTS, &objects)?;
    let graphql_typename = Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT graphql_typename FROM {} WHERE entity = $1",
                    crate::utils::meta_table()
                ),
                None,
                &[crate::utils::spi::text(entity)],
            )?
            .first()
            .get_one::<String>()
    })
    .map_err(|e| crate::utils::spi::catalog_error("Read the GraphQL type name", &e))?;
    let user_indexes = user_indexes(meta.tview_oid)?;

    crate::ddl::drop::drop_tview(
        &format!("{}.{tv_name}", quote_identifier(schema)),
        false,
        false,
    )?;
    create::without_rows(|| {
        create::create_tview_in(
            &tv_name,
            query,
            schema,
            group_keys,
            storage,
            Some(declarations),
        )
    })?;

    let rebuilt =
        TviewMeta::load_by_entity(entity)?.ok_or_else(|| TViewError::MetadataNotFound {
            entity: entity.to_string(),
        })?;
    let (tv, view) = (
        format!(
            "{}.{}",
            quote_identifier(schema),
            quote_identifier(&tv_name)
        ),
        crate::utils::qualified_relname_from_oid(rebuilt.view_oid)?,
    );
    restore_privileges(&restore, &tv, &view)?;
    crate::ddl::privileges::follow(Some(rebuilt.tview_oid), true)?;
    if let Some(typename) = graphql_typename {
        restore_typename(entity, &typename)?;
    }
    recreate_user_indexes(&tv_name, user_indexes)?;
    // Filled as its owner, now that the owner is back.
    crate::admin::fill_empty_tview(entity)
}

/// Run the saved `restore` statements over the rebuilt table `tv` and view `view`:
/// owners first; then the new objects' default privileges give way to the saved
/// ones; then comments. The hook must not follow the table midway; the backing
/// view follows the table's owner and grants afterwards.
fn restore_privileges(restore: &[String], tv: &str, view: &str) -> TViewResult<()> {
    let _internal = crate::internal_ddl::InternalDdl::begin();
    let (owners, others): (Vec<&String>, Vec<&String>) =
        restore.iter().partition(|s| s.starts_with("ALTER "));
    for statement in owners {
        crate::utils::spi::run_ddl(statement)?;
    }
    for revoke in crate::utils::spi::strings(
        "SELECT pg_catalog.format('REVOKE ALL ON %s FROM %s', c.oid::pg_catalog.regclass, \
             CASE WHEN a.grantee = 0 THEN 'PUBLIC' \
                  ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(a.grantee)) END) \
         FROM pg_catalog.pg_class c, pg_catalog.aclexplode(c.relacl) a \
         WHERE c.oid IN ($1::pg_catalog.regclass, $2::pg_catalog.regclass) \
           AND a.grantee <> c.relowner \
         GROUP BY c.oid, a.grantee",
        &[crate::utils::spi::text(tv), crate::utils::spi::text(view)],
    )? {
        crate::utils::spi::run_ddl(&revoke)?;
    }
    for statement in others {
        crate::utils::spi::run_ddl(statement)?;
    }
    Ok(())
}

/// Give the rebuilt TVIEW of `entity` back its GraphQL type name.
fn restore_typename(entity: &str, typename: &str) -> TViewResult<()> {
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
    .map_err(|e| crate::utils::spi::catalog_error("Restore the GraphQL type name", &e))
}

/// Re-create the user's indexes on the rebuilt `tv_name`, each in a block that
/// names the index when it no longer applies.
fn recreate_user_indexes(tv_name: &str, indexes: Vec<(String, String)>) -> TViewResult<()> {
    for (index, definition) in indexes {
        let wrapped = Spi::connect(|client| {
            client
                .select(
                    "SELECT pg_catalog.format('DO %L', pg_catalog.format(\
                         'BEGIN EXECUTE %L; EXCEPTION WHEN OTHERS THEN RAISE EXCEPTION \
                          USING MESSAGE = %L || SQLERRM, ERRCODE = SQLSTATE; END', $1, $2))",
                    None,
                    &[
                        crate::utils::spi::text(&definition),
                        crate::utils::spi::text(format!(
                            "index {index} on {tv_name} cannot be re-created after the \
                             rebuild: "
                        )),
                    ],
                )?
                .first()
                .get_one::<String>()
        })
        .map_err(|e| crate::utils::spi::catalog_error("Prepare a user index", &e))?
        .unwrap_or_default();
        crate::utils::spi::run_ddl(&wrapped)?;
    }
    Ok(())
}

/// Why a TVIEW (`$1` its table, `$2` its backing view) cannot be rebuilt: objects
/// that depend on it, and table properties a rebuild would drop.
pub(super) const REBUILD_REFUSALS: &str = "\
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

/// Statements that give the rebuilt table (`$1`) the owner and privileges it
/// has now, and it and its view (`$2`) their comments, in the order to run them.
/// The view's owner and privileges follow the table's.
pub(super) const RESTORE_STATEMENTS: &str = "\
    WITH objects(kind, relid) AS (VALUES ('TABLE', $1), ('VIEW', $2)) \
    SELECT statement FROM ( \
        SELECT 1 AS step, pg_catalog.format('ALTER %s %s OWNER TO %I', o.kind, \
               c.oid::pg_catalog.regclass, pg_catalog.pg_get_userbyid(c.relowner)) AS statement \
        FROM objects o JOIN pg_catalog.pg_class c ON c.oid = o.relid \
        WHERE o.kind = 'TABLE' \
          AND c.relowner <> (SELECT oid FROM pg_catalog.pg_roles WHERE rolname = CURRENT_USER) \
      UNION ALL \
        SELECT 2, pg_catalog.format('GRANT %s ON %s TO %s%s', a.privilege_type, \
               c.oid::pg_catalog.regclass, \
               CASE WHEN a.grantee = 0 THEN 'PUBLIC' \
                    ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(a.grantee)) END, \
               CASE WHEN a.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END) \
        FROM objects o JOIN pg_catalog.pg_class c ON c.oid = o.relid, \
             pg_catalog.aclexplode(c.relacl) a \
        WHERE o.kind = 'TABLE' AND a.grantee <> c.relowner \
      UNION ALL \
        SELECT 3, pg_catalog.format('COMMENT ON %s %s IS %L', o.kind, \
               c.oid::pg_catalog.regclass, d.description) \
        FROM objects o JOIN pg_catalog.pg_class c ON c.oid = o.relid \
        JOIN pg_catalog.pg_description d ON d.objoid = c.oid \
         AND d.classoid = 'pg_catalog.pg_class'::pg_catalog.regclass AND d.objsubid = 0 \
    ) s ORDER BY step, statement";

/// Indexes a user added to the TVIEW's table, as `(name, definition)`: every
/// index that backs no constraint and that `pg_tviews` did not create.
pub(super) fn user_indexes(table: pg_sys::Oid) -> TViewResult<Vec<(String, String)>> {
    Spi::connect(|client| {
        let mut indexes = Vec::new();
        for row in client.select(
            &format!(
                "SELECT ic.relname::text, pg_catalog.pg_get_indexdef(i.indexrelid) \
                 FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid \
                 WHERE i.indrelid = $1 \
                   AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k \
                                   WHERE k.conindid = i.indexrelid) \
                   AND ic.relname <> ALL (SELECT pg_catalog.unnest(m.managed_index_names) \
                                          FROM {} m \
                                          WHERE m.table_oid = $1::pg_catalog.regclass) \
                 ORDER BY 1",
                crate::utils::meta_table()
            ),
            None,
            &[crate::utils::spi::oid(table)],
        )? {
            if let (Some(name), Some(definition)) = (row.get::<String>(1)?, row.get::<String>(2)?) {
                indexes.push((name, definition));
            }
        }
        Ok::<_, spi::Error>(indexes)
    })
    .map_err(|e| crate::utils::spi::catalog_error("List the TVIEW's indexes", &e))
}

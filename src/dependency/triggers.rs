use crate::error::{TViewError, TViewResult};
use crate::utils::quote_identifier;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Row-level trigger function: enqueues refreshes.
const ROW_HANDLER: &str = "pg_tview_trigger_handler";
/// Statement-level trigger function: flushes the refresh queue.
const FLUSH_HANDLER: &str = "pg_tview_flush_trigger";

/// The two triggers each base table gets: `(function, level, name tag)`.
const TRIGGERS: [(&str, &str, &str); 2] = [
    (ROW_HANDLER, "ROW", "row"),
    (FLUSH_HANDLER, "STATEMENT", "flush"),
];

/// Name of a trigger for `entity` on `schema.relname`:
/// `trg_tview_<tag>_<entity>_on_<schema>_<table>`, fitted to 63 bytes. The tag
/// comes right after the fixed prefix, so the row trigger of one entity never
/// has the name of another entity's flush trigger.
fn trigger_name(tag: &str, entity: &str, schema: &str, relname: &str) -> String {
    crate::utils::fit_identifier(format!("trg_tview_{tag}_{entity}_on_{schema}_{relname}"))
}

/// A `pg_tviews` trigger on a base table.
struct InstalledTrigger {
    table_oid: pg_sys::Oid,
    /// Quoted, schema-qualified table.
    table: String,
    trigger: String,
    function: String,
}

/// The `pg_tviews` triggers installed for `entity`, on `table_oid` only when given.
///
/// A trigger is recognised by its function (`tgfoid`) and the entity it carries
/// as its argument, not by its name, which a rename of the table or its schema
/// leaves stale.
fn entity_triggers(
    entity: &str,
    table_oid: Option<pg_sys::Oid>,
) -> TViewResult<Vec<InstalledTrigger>> {
    let query = format!(
        "SELECT pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname), \
                t.tgname::text, p.proname::text, t.tgrelid \
         FROM pg_catalog.pg_trigger t \
         JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
         JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE p.pronamespace = '{schema}'::pg_catalog.regnamespace \
           AND p.proname IN ('{ROW_HANDLER}', '{FLUSH_HANDLER}') \
           AND t.tgnargs = 1 \
           AND t.tgargs = pg_catalog.convert_to($1, pg_catalog.getdatabaseencoding()) \
                          || pg_catalog.decode('00', 'hex') \
           AND ($2 IS NULL OR t.tgrelid = $2)",
        schema = crate::utils::ext_schema(),
    );
    Spi::connect(|client| {
        // SAFETY: the datums borrow `entity` and copy `table_oid`, both outliving
        // the select.
        let args = [
            unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(table_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) },
        ];
        let mut found = Vec::new();
        for row in client.select(&query, None, &args)? {
            if let (Some(table), Some(trigger), Some(function), Some(table_oid)) = (
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<String>(3)?,
                row.get::<pg_sys::Oid>(4)?,
            ) {
                found.push(InstalledTrigger {
                    table_oid,
                    table,
                    trigger,
                    function,
                });
            }
        }
        Ok::<_, spi::Error>(found)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Find triggers of TVIEW {entity}"),
        pg_error: e.to_string(),
    })
}

/// `pg_tviews` triggers that no registered TVIEW accounts for, as
/// `<trigger> on <table>`: the entity a trigger carries is not registered, or its
/// backing view does not read the trigger's table. A TVIEW's tables are the
/// ordinary and partitioned tables reached from its backing view through views,
/// other TVIEWs' tables excepted, as [`install_triggers`] was given them.
///
/// # Errors
/// Returns an error if the catalog query fails.
pub fn orphaned_triggers() -> TViewResult<Vec<String>> {
    let query = format!(
        "WITH RECURSIVE reads(entity, relid) AS ( \
             SELECT m.entity, m.view_oid::oid FROM {meta} m \
           UNION \
             SELECT r.entity, d.refobjid \
             FROM reads r \
             JOIN pg_catalog.pg_class v ON v.oid = r.relid AND v.relkind = 'v' \
             JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid \
             JOIN pg_catalog.pg_depend d \
               ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass \
              AND d.objid = w.oid \
              AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
              AND d.refobjid <> v.oid \
         ), \
         ours AS ( \
             SELECT t.tgname, t.tgrelid, \
                    CASE WHEN t.tgnargs = 1 THEN pg_catalog.convert_from( \
                        pg_catalog.substring(t.tgargs, 1, pg_catalog.length(t.tgargs) - 1), \
                        pg_catalog.getdatabaseencoding()) END AS entity \
             FROM pg_catalog.pg_trigger t \
             JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
             WHERE p.pronamespace = '{schema}'::pg_catalog.regnamespace \
               AND p.proname IN ('{ROW_HANDLER}', '{FLUSH_HANDLER}') \
         ) \
         SELECT pg_catalog.format('%I on %s', o.tgname, o.tgrelid::pg_catalog.regclass) \
                AS orphan \
         FROM ours o \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM reads r \
             JOIN pg_catalog.pg_class c ON c.oid = r.relid AND c.relkind IN ('r', 'p') \
             WHERE r.entity = o.entity AND r.relid = o.tgrelid \
               AND r.relid NOT IN (SELECT table_oid::oid FROM {meta})) \
         ORDER BY 1",
        meta = crate::utils::meta_table(),
        schema = crate::utils::ext_schema(),
    );
    Spi::connect(|client| {
        let mut orphans = Vec::new();
        for row in client.select(&query, None, &[])? {
            if let Some(orphan) = row["orphan"].value::<String>()? {
                orphans.push(orphan);
            }
        }
        Ok::<_, spi::Error>(orphans)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Find orphaned pg_tviews triggers".to_string(),
        pg_error: e.to_string(),
    })
}

/// Install cascade triggers on all base tables for a TVIEW.
///
/// Each base table gets a row-level `pg_tview_trigger_handler()`, which derives
/// the entity from the table OID via an internal cache and enqueues a refresh,
/// and a statement-level `pg_tview_flush_trigger()`, which flushes the queue so
/// auto-commit statements refresh too (the `ProcessUtility` hook flushes on an
/// explicit COMMIT). Both are called schema-qualified and carry the entity as
/// their argument, which is how [`remove_entity_triggers`] finds them. A trigger
/// already there is left alone.
///
/// # Errors
/// Returns error if trigger creation or installation fails.
pub fn install_triggers(table_oids: &[pg_sys::Oid], tview_entity: &str) -> TViewResult<()> {
    // A trigger argument written as a quoted identifier is stored as its name.
    let entity_arg = quote_identifier(tview_entity);
    for &table_oid in table_oids {
        let (schema, relname) = get_table_name(table_oid)?;
        let qi_table = format!(
            "{}.{}",
            quote_identifier(&schema),
            quote_identifier(&relname)
        );
        let installed = entity_triggers(tview_entity, Some(table_oid))?;

        for (function, level, tag) in TRIGGERS {
            if installed.iter().any(|t| t.function == function) {
                continue;
            }
            let trigger_sql = format!(
                "CREATE TRIGGER {}
                 AFTER INSERT OR UPDATE OR DELETE ON {qi_table}
                 FOR EACH {level}
                 EXECUTE FUNCTION {}.{function}({entity_arg})",
                quote_identifier(&trigger_name(tag, tview_entity, &schema, &relname)),
                crate::utils::ext_schema(),
            );
            crate::utils::spi_run_ddl(&trigger_sql).map_err(|e| TViewError::CatalogError {
                operation: format!("Install {function} trigger on {qi_table}"),
                pg_error: e,
            })?;
        }
    }

    Ok(())
}

/// Remove every trigger installed for `tview_entity`, wherever it is. This needs
/// no dependency walk, so it still works once the backing view is gone (a base
/// table or helper view dropped with CASCADE).
///
/// # Errors
/// Returns an error if the catalog query or a trigger drop fails.
pub fn remove_entity_triggers(tview_entity: &str) -> TViewResult<()> {
    for installed in entity_triggers(tview_entity, None)? {
        drop_trigger(installed.table_oid, &installed.table, &installed.trigger)?;
    }
    Ok(())
}

/// `DROP TRIGGER IF EXISTS trigger ON table` (`table` quoted and qualified), as
/// the table's owner: `DROP TRIGGER` needs the owner where `CREATE TRIGGER` needs
/// only the `TRIGGER` privilege, and these are `pg_tviews`' own triggers, removed
/// for a TVIEW the caller may drop (issue #136).
fn drop_trigger(table_oid: pg_sys::Oid, table: &str, trigger: &str) -> TViewResult<()> {
    let _owner = crate::owner::AsOwner::of_table(table_oid)?;
    let drop_sql = format!(
        "DROP TRIGGER IF EXISTS {} ON {table}",
        quote_identifier(trigger)
    );
    crate::utils::spi_run_ddl(&drop_sql).map_err(|e| TViewError::CatalogError {
        operation: format!("Drop trigger {trigger} from {table}"),
        pg_error: e,
    })
}

/// Migrate all existing triggers from the old PL/pgSQL `tview_trigger_handler()`
/// to the Rust `pg_tview_trigger_handler()`.
///
/// Iterates over every `(entity, dependency)` pair in `pg_tview_meta`, drops the
/// table's legacy triggers (those of the PL/pgSQL handler, and `pg_tviews` triggers
/// that carry no entity), and installs the entity's triggers. Legacy triggers are
/// found by their function, never by name, so no current trigger of another
/// entity is touched. The operation is idempotent.
///
/// # Errors
/// Returns error if any trigger drop or creation fails.
pub fn migrate_all_triggers_to_rust_handler() -> TViewResult<()> {
    // Collect (entity, table_oid) pairs from pg_tview_meta
    let pairs: Vec<(String, pg_sys::Oid)> = Spi::connect(|client| {
        let rows = client.select(
            &format!(
                "SELECT m.entity, d.refobjid::oid AS table_oid \
                 FROM {} m \
                 JOIN pg_depend d ON d.objid = m.view_oid \
                 JOIN pg_class c ON c.oid = d.refobjid AND c.relkind = 'r' \
                 WHERE d.deptype = 'n'",
                crate::utils::meta_table()
            ),
            None,
            &[],
        )?;

        let mut out = Vec::new();
        for row in rows {
            let entity: String = row["entity"].value()?.ok_or_else(|| {
                spi::Error::from(TViewError::SpiError {
                    query: "migrate: SELECT entity".to_string(),
                    error: "entity column is NULL".to_string(),
                })
            })?;
            let table_oid: pg_sys::Oid = row["table_oid"].value()?.ok_or_else(|| {
                spi::Error::from(TViewError::SpiError {
                    query: "migrate: SELECT table_oid".to_string(),
                    error: "table_oid column is NULL".to_string(),
                })
            })?;
            out.push((entity, table_oid));
        }
        Ok(out)
    })
    .map_err(|e: spi::Error| TViewError::CatalogError {
        operation: "Migrate triggers: read pg_tview_meta".to_string(),
        pg_error: format!("{e:?}"),
    })?;

    for (entity, table_oid) in pairs {
        for (table, trigger) in legacy_triggers(table_oid)? {
            drop_trigger(table_oid, &table, &trigger)?;
        }
        install_triggers(&[table_oid], &entity)?;
    }

    Ok(())
}

/// Legacy triggers on `table_oid`, as `(quoted table, trigger)`: those calling a
/// `tview_trigger_handler()` (the old PL/pgSQL handler) and `pg_tviews` triggers
/// installed without the entity argument.
fn legacy_triggers(table_oid: pg_sys::Oid) -> TViewResult<Vec<(String, String)>> {
    let query = format!(
        "SELECT pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname), \
                t.tgname::text \
         FROM pg_catalog.pg_trigger t \
         JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
         JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE t.tgrelid = $1 AND NOT t.tgisinternal \
           AND (p.proname = 'tview_trigger_handler' \
                OR (p.pronamespace = '{schema}'::pg_catalog.regnamespace \
                    AND p.proname IN ('{ROW_HANDLER}', '{FLUSH_HANDLER}') \
                    AND t.tgnargs = 0))",
        schema = crate::utils::ext_schema(),
    );
    Spi::connect(|client| {
        // SAFETY: the datum copies `table_oid`.
        let args = [unsafe {
            DatumWithOid::new(table_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value())
        }];
        let mut found = Vec::new();
        for row in client.select(&query, None, &args)? {
            if let (Some(table), Some(trigger)) = (row.get::<String>(1)?, row.get::<String>(2)?) {
                found.push((table, trigger));
            }
        }
        Ok::<_, spi::Error>(found)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Find legacy triggers on {table_oid:?}"),
        pg_error: e.to_string(),
    })
}

/// Returns `(schema_name, table_name)` for the given OID, both unquoted.
fn get_table_name(oid: pg_sys::Oid) -> TViewResult<(String, String)> {
    let names = Spi::connect(|client| {
        // SAFETY: the datum copies `oid`.
        let args =
            [unsafe { DatumWithOid::new(oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }];
        client
            .select(
                "SELECT n.nspname::text, c.relname::text \
                 FROM pg_catalog.pg_class c \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 WHERE c.oid = $1",
                None,
                &args,
            )?
            .first()
            .get_two::<String, String>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get table name for OID {oid:?}"),
        pg_error: e.to_string(),
    })?;
    match names {
        (Some(schema), Some(relname)) => Ok((schema, relname)),
        _ => Err(TViewError::DependencyResolutionFailed {
            view_name: format!("OID {oid:?}"),
            reason: "Table not found".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::trigger_name;
    use crate::utils::MAX_IDENTIFIER_BYTES;

    #[test]
    fn test_trigger_name_short_is_verbatim() {
        assert_eq!(
            trigger_name("row", "post", "public", "tb_user"),
            "trg_tview_row_post_on_public_tb_user"
        );
        assert_eq!(
            trigger_name("flush", "post", "public", "tb_user"),
            "trg_tview_flush_post_on_public_tb_user"
        );
    }

    #[test]
    fn test_trigger_name_row_and_flush_of_other_entities_differ() {
        // Entity `flush_x`'s row trigger and entity `x`'s flush trigger.
        assert_ne!(
            trigger_name("row", "flush_x", "public", "tb_t"),
            trigger_name("flush", "x", "public", "tb_t")
        );
    }

    #[test]
    fn test_trigger_name_long_prefixes_stay_distinct() {
        let common = "invoice_line_adjustment_with_a_deliberately_long_name_";
        let a = trigger_name("row", &format!("{common}a"), "app", "tb_x");
        let b = trigger_name("row", &format!("{common}b"), "app", "tb_x");
        assert_eq!(a.len(), MAX_IDENTIFIER_BYTES);
        assert_ne!(a, b);
    }

    #[test]
    fn test_trigger_name_counts_bytes() {
        let name = trigger_name("row", "note", &"é".repeat(40), "tb_note");
        assert!(name.len() <= MAX_IDENTIFIER_BYTES);
    }
}

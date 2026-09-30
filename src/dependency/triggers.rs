use crate::error::{TViewError, TViewResult};
use crate::utils::quote_identifier;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Row-level trigger function: enqueues refreshes.
const ROW_HANDLER: &str = "pg_tview_trigger_handler";
/// Statement-level trigger function: flushes the refresh queue.
const FLUSH_HANDLER: &str = "pg_tview_flush_trigger";

/// Name of the trigger `function` installs for `entity` on `schema.relname`:
/// `trg_tview[_flush]_<entity>_on_<schema>_<table>`, fitted to 63 bytes.
fn trigger_name(function: &str, entity: &str, schema: &str, relname: &str) -> String {
    let kind = if function == FLUSH_HANDLER {
        "flush_"
    } else {
        ""
    };
    crate::utils::fit_identifier(format!("trg_tview_{kind}{entity}_on_{schema}_{relname}"))
}

/// The `pg_tviews` triggers installed for `entity`: `(table, trigger, function)`,
/// on `table_oid` only when given.
///
/// A trigger is recognised by its function (`tgfoid`) and the entity it carries
/// as its argument, not by its name, which a rename of the table or its schema
/// leaves stale.
fn entity_triggers(
    entity: &str,
    table_oid: Option<pg_sys::Oid>,
) -> TViewResult<Vec<(pg_sys::Oid, String, String)>> {
    let query = format!(
        "SELECT t.tgrelid AS table_oid, t.tgname::text AS trigger, p.proname::text AS function \
         FROM pg_catalog.pg_trigger t \
         JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
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
            if let (Some(table), Some(trigger), Some(function)) = (
                row["table_oid"].value::<pg_sys::Oid>()?,
                row["trigger"].value::<String>()?,
                row["function"].value::<String>()?,
            ) {
                found.push((table, trigger, function));
            }
        }
        Ok::<_, spi::Error>(found)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Find triggers of TVIEW {entity}"),
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
/// their argument, which is how [`remove_entity_triggers`] finds them.
///
/// # Errors
/// Returns error if trigger creation or installation fails.
pub fn install_triggers(table_oids: &[pg_sys::Oid], tview_entity: &str) -> TViewResult<()> {
    let entity_arg = format!("'{}'", tview_entity.replace('\'', "''"));
    for &table_oid in table_oids {
        let (schema, relname) = get_table_name(table_oid)?;
        // Schema-qualified SQL reference: "schema"."table"
        let qi_table = format!(
            "{}.{}",
            quote_identifier(&schema),
            quote_identifier(&relname)
        );
        let installed = entity_triggers(tview_entity, Some(table_oid))?;

        for (function, level) in [(ROW_HANDLER, "ROW"), (FLUSH_HANDLER, "STATEMENT")] {
            if let Some((_, trigger, _)) = installed.iter().find(|(_, _, f)| f == function) {
                if function == ROW_HANDLER {
                    warning!("Trigger {trigger} already exists on {schema}.{relname}, skipping");
                }
                continue;
            }
            let trigger_sql = format!(
                "CREATE TRIGGER {}
                 AFTER INSERT OR UPDATE OR DELETE ON {qi_table}
                 FOR EACH {level}
                 EXECUTE FUNCTION {}.{function}({entity_arg})",
                quote_identifier(&trigger_name(function, tview_entity, &schema, &relname)),
                crate::utils::ext_schema(),
            );
            crate::utils::spi_run_ddl(&trigger_sql).map_err(|e| TViewError::CatalogError {
                operation: format!("Install {function} trigger on {schema}.{relname}"),
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
    for (table_oid, trigger, _) in entity_triggers(tview_entity, None)? {
        drop_trigger(table_oid, &trigger)?;
    }
    Ok(())
}

/// `DROP TRIGGER IF EXISTS trigger ON table`.
fn drop_trigger(table_oid: pg_sys::Oid, trigger: &str) -> TViewResult<()> {
    let (schema, relname) = get_table_name(table_oid)?;
    let drop_sql = format!(
        "DROP TRIGGER IF EXISTS {} ON {}.{}",
        quote_identifier(trigger),
        quote_identifier(&schema),
        quote_identifier(&relname)
    );
    crate::utils::spi_run_ddl(&drop_sql).map_err(|e| TViewError::CatalogError {
        operation: format!("Drop trigger {trigger} from {schema}.{relname}"),
        pg_error: e,
    })
}

/// Migrate all existing triggers from the old PL/pgSQL `tview_trigger_handler()`
/// to the Rust `pg_tview_trigger_handler()`.
///
/// Iterates over every `(entity, dependency)` pair in `pg_tview_meta`, drops the
/// entity's triggers on that table (old or current), and installs them again.
/// The operation is idempotent.
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
        // Triggers of the PL/pgSQL handler carry the untagged legacy names;
        // current ones are found by function and entity.
        let (schema, relname) = get_table_name(table_oid)?;
        for legacy in [
            format!("trg_tview_{entity}_on_{schema}_{relname}"),
            format!("trg_tview_flush_{entity}_on_{schema}_{relname}"),
        ] {
            drop_trigger(table_oid, &legacy)?;
        }
        for (_, trigger, _) in entity_triggers(&entity, Some(table_oid))? {
            drop_trigger(table_oid, &trigger)?;
        }
        install_triggers(&[table_oid], &entity)?;
    }

    Ok(())
}

/// Returns `(schema_name, table_name)` for the given OID.
///
/// Both parts are unquoted identifiers.  Build the SQL reference as
/// `quote_identifier(schema) + "." + quote_identifier(table)`.
fn get_table_name(oid: pg_sys::Oid) -> TViewResult<(String, String)> {
    let row = crate::utils::spi_get_string(&format!(
        "SELECT n.nspname::text || ':' || c.relname::text \
         FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.oid = {oid:?}"
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get table name for OID {oid:?}"),
        pg_error: format!("{e:?}"),
    })?
    .ok_or_else(|| TViewError::DependencyResolutionFailed {
        view_name: format!("OID {oid:?}"),
        reason: "Table not found".to_string(),
    })?;

    // Split on the sentinel ':' — safe because PostgreSQL identifiers never
    // contain ':'.
    let (schema, relname) =
        row.split_once(':')
            .ok_or_else(|| TViewError::DependencyResolutionFailed {
                view_name: format!("OID {oid:?}"),
                reason: "Unexpected format from pg_class lookup".to_string(),
            })?;
    Ok((schema.to_string(), relname.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{FLUSH_HANDLER, ROW_HANDLER, trigger_name};
    use crate::utils::MAX_IDENTIFIER_BYTES;

    #[test]
    fn test_trigger_name_short_is_verbatim() {
        assert_eq!(
            trigger_name(ROW_HANDLER, "post", "public", "tb_user"),
            "trg_tview_post_on_public_tb_user"
        );
        assert_eq!(
            trigger_name(FLUSH_HANDLER, "post", "public", "tb_user"),
            "trg_tview_flush_post_on_public_tb_user"
        );
    }

    #[test]
    fn test_trigger_name_long_prefixes_stay_distinct() {
        let common = "invoice_line_adjustment_with_a_deliberately_long_name_";
        let a = trigger_name(ROW_HANDLER, &format!("{common}a"), "app", "tb_x");
        let b = trigger_name(ROW_HANDLER, &format!("{common}b"), "app", "tb_x");
        assert_eq!(a.len(), MAX_IDENTIFIER_BYTES);
        assert_ne!(a, b);
    }

    #[test]
    fn test_trigger_name_counts_bytes() {
        let name = trigger_name(ROW_HANDLER, "note", &"é".repeat(40), "tb_note");
        assert!(name.len() <= MAX_IDENTIFIER_BYTES);
    }
}

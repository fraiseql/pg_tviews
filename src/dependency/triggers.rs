use crate::error::{TViewError, TViewResult};
use crate::utils::quote_identifier;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Install cascade triggers on all base tables for a TVIEW.
///
/// Triggers point at the Rust `pg_tview_trigger_handler()` (`#[pg_trigger]`),
/// which derives the entity from the table OID via an internal cache and
/// enqueues a refresh into the transaction-local queue. This avoids the nested
/// SPI issue that the old PL/pgSQL `tview_trigger_handler()` suffered from.
///
/// # Errors
/// Returns error if trigger creation or installation fails.
pub fn install_triggers(table_oids: &[pg_sys::Oid], tview_entity: &str) -> TViewResult<()> {
    // Install trigger on each base table
    for &table_oid in table_oids {
        let (schema, relname) = get_table_name(table_oid)?;
        // Schema-qualified SQL reference: "schema"."table"
        let qi_table = format!(
            "{}.{}",
            quote_identifier(&schema),
            quote_identifier(&relname)
        );
        // Trigger name uses schema_table to stay free of dots
        let trigger_suffix = format!("{schema}_{relname}");

        // Use deterministic trigger name: trg_tview_{entity}_on_{schema}_{table}
        let trigger_name = format!("trg_tview_{tview_entity}_on_{trigger_suffix}");
        let qi_trigger = quote_identifier(&trigger_name);

        // Check if trigger already exists
        if trigger_exists_by_oid(table_oid, &trigger_name)? {
            warning!(
                "Trigger {} already exists on {}.{}, skipping",
                trigger_name,
                schema,
                relname
            );
            continue;
        }

        // Install AFTER INSERT OR UPDATE OR DELETE trigger (row-level: enqueues refreshes)
        // The Rust handler derives the entity from the table OID — no argument needed
        let trigger_sql = format!(
            "CREATE TRIGGER {qi_trigger}
             AFTER INSERT OR UPDATE OR DELETE ON {qi_table}
             FOR EACH ROW
             EXECUTE FUNCTION pg_tview_trigger_handler()"
        );

        crate::utils::spi_run_ddl(&trigger_sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Install trigger on {schema}.{relname}"),
            pg_error: e,
        })?;

        // Install statement-level AFTER trigger (flushes the refresh queue).
        // This ensures auto-commit transactions get TVIEWs refreshed.
        // For explicit transactions, the ProcessUtility hook also flushes on COMMIT.
        let flush_trigger_name = format!("trg_tview_flush_{tview_entity}_on_{trigger_suffix}");
        let qi_flush_trigger = quote_identifier(&flush_trigger_name);
        if !trigger_exists_by_oid(table_oid, &flush_trigger_name)? {
            let flush_sql = format!(
                "CREATE TRIGGER {qi_flush_trigger}
                 AFTER INSERT OR UPDATE OR DELETE ON {qi_table}
                 FOR EACH STATEMENT
                 EXECUTE FUNCTION pg_tview_flush_trigger()"
            );
            crate::utils::spi_run_ddl(&flush_sql).map_err(|e| TViewError::CatalogError {
                operation: format!("Install flush trigger on {schema}.{relname}"),
                pg_error: e,
            })?;
        }
    }

    Ok(())
}

/// Remove cascade triggers from all base tables for a TVIEW.
///
/// # Errors
/// Returns error if trigger removal fails.
pub fn remove_triggers(table_oids: &[pg_sys::Oid], tview_entity: &str) -> TViewResult<()> {
    for &table_oid in table_oids {
        let (schema, relname) = get_table_name(table_oid)?;
        let qi_table = format!(
            "{}.{}",
            quote_identifier(&schema),
            quote_identifier(&relname)
        );
        let trigger_suffix = format!("{schema}_{relname}");
        let trigger_name = format!("trg_tview_{tview_entity}_on_{trigger_suffix}");
        let flush_trigger_name = format!("trg_tview_flush_{tview_entity}_on_{trigger_suffix}");

        let drop_sql = format!(
            "DROP TRIGGER IF EXISTS {} ON {qi_table}",
            quote_identifier(&trigger_name),
        );
        crate::utils::spi_run_ddl(&drop_sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Drop trigger from {schema}.{relname}"),
            pg_error: e,
        })?;

        let drop_flush_sql = format!(
            "DROP TRIGGER IF EXISTS {} ON {qi_table}",
            quote_identifier(&flush_trigger_name),
        );
        crate::utils::spi_run_ddl(&drop_flush_sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Drop flush trigger from {schema}.{relname}"),
            pg_error: e,
        })?;
    }

    Ok(())
}

/// Migrate all existing triggers from the old PL/pgSQL `tview_trigger_handler()`
/// to the Rust `pg_tview_trigger_handler()`.
///
/// Iterates over every `(entity, dependency)` pair in `pg_tview_meta`, drops the
/// old trigger (if present), and recreates it pointing at the Rust handler.
/// The operation is idempotent: triggers already pointing at the Rust handler
/// are recreated harmlessly, and missing triggers are simply created.
///
/// # Errors
/// Returns error if any trigger drop or creation fails.
pub fn migrate_all_triggers_to_rust_handler() -> TViewResult<()> {
    // Collect (entity, table_oid) pairs from pg_tview_meta
    let pairs: Vec<(String, pg_sys::Oid)> = Spi::connect(|client| {
        let rows = client.select(
            "SELECT entity, unnest(dependencies) AS table_oid FROM pg_tview_meta",
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
        let (schema, relname) = get_table_name(table_oid)?;
        let qi_table = format!(
            "{}.{}",
            quote_identifier(&schema),
            quote_identifier(&relname)
        );
        let trigger_suffix = format!("{schema}_{relname}");
        let trigger_name = format!("trg_tview_{entity}_on_{trigger_suffix}");
        let qi_trigger = quote_identifier(&trigger_name);

        // Drop the old trigger (IF EXISTS makes this safe if already removed)
        let drop_sql = format!("DROP TRIGGER IF EXISTS {qi_trigger} ON {qi_table}");
        crate::utils::spi_run_ddl(&drop_sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Migrate trigger: drop {trigger_name} on {schema}.{relname}"),
            pg_error: e,
        })?;

        // Recreate pointing at the Rust handler
        let create_sql = format!(
            "CREATE TRIGGER {qi_trigger}
             AFTER INSERT OR UPDATE OR DELETE ON {qi_table}
             FOR EACH ROW
             EXECUTE FUNCTION pg_tview_trigger_handler()"
        );
        crate::utils::spi_run_ddl(&create_sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Migrate trigger: create {trigger_name} on {schema}.{relname}"),
            pg_error: e,
        })?;

        // Install statement-level flush trigger
        let flush_trigger_name = format!("trg_tview_flush_{entity}_on_{trigger_suffix}");
        let qi_flush = quote_identifier(&flush_trigger_name);
        let drop_flush = format!("DROP TRIGGER IF EXISTS {qi_flush} ON {qi_table}");
        crate::utils::spi_run_ddl(&drop_flush).map_err(|e| TViewError::CatalogError {
            operation: format!("Migrate: drop flush trigger {flush_trigger_name}"),
            pg_error: e,
        })?;

        let create_flush = format!(
            "CREATE TRIGGER {qi_flush}
             AFTER INSERT OR UPDATE OR DELETE ON {qi_table}
             FOR EACH STATEMENT
             EXECUTE FUNCTION pg_tview_flush_trigger()"
        );
        crate::utils::spi_run_ddl(&create_flush).map_err(|e| TViewError::CatalogError {
            operation: format!("Migrate: create flush trigger {flush_trigger_name}"),
            pg_error: e,
        })?;
    }

    Ok(())
}

/// Returns `(schema_name, table_name)` for the given OID.
///
/// Both parts are unquoted identifiers.  Build the SQL reference as
/// `quote_identifier(schema) + "." + quote_identifier(table)`, and the
/// trigger name suffix as `schema + "_" + table` (dots are not valid in
/// PostgreSQL trigger names).
fn get_table_name(oid: pg_sys::Oid) -> TViewResult<(String, String)> {
    let row = crate::utils::spi_get_string(&format!(
        "SELECT n.nspname || ':' || c.relname \
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

/// Check if a trigger exists on a table identified by OID, to avoid search_path
/// sensitivity of the `::regclass` cast.
fn trigger_exists_by_oid(table_oid: pg_sys::Oid, trigger_name: &str) -> TViewResult<bool> {
    let args = vec![unsafe {
        DatumWithOid::new(trigger_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
    }];
    Spi::get_one_with_args::<bool>(
        &format!(
            "SELECT COUNT(*) > 0 FROM pg_trigger \
             WHERE tgrelid = {table_oid:?} \
               AND tgname = $1"
        ),
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check trigger {trigger_name}"),
        pg_error: format!("{e:?}"),
    })
    .map(|opt| opt.unwrap_or(false))
}

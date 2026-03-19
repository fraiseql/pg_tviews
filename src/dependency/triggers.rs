use pgrx::prelude::*;
use pgrx::datum::DatumWithOid;
use crate::error::{TViewError, TViewResult};
use crate::refresh::bulk::quote_identifier;

/// Install cascade triggers on all base tables for a TVIEW.
///
/// Triggers point at the Rust `pg_tview_trigger_handler()` (`#[pg_trigger]`),
/// which derives the entity from the table OID via an internal cache and
/// enqueues a refresh into the transaction-local queue. This avoids the nested
/// SPI issue that the old PL/pgSQL `tview_trigger_handler()` suffered from.
///
/// # Errors
/// Returns error if trigger creation or installation fails.
pub fn install_triggers(
    table_oids: &[pg_sys::Oid],
    tview_entity: &str,
) -> TViewResult<()> {
    // Install trigger on each base table
    for &table_oid in table_oids {
        let table_name = get_table_name(table_oid)?;
        let qi_table = quote_identifier(&table_name);

        // Use deterministic trigger name: trg_tview_{entity}_on_{table}
        let trigger_name = format!("trg_tview_{tview_entity}_on_{table_name}");
        let qi_trigger = quote_identifier(&trigger_name);

        // Check if trigger already exists
        if trigger_exists(&table_name, &trigger_name)? {
            warning!("Trigger {} already exists on {}, skipping", trigger_name, table_name);
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
            operation: format!("Install trigger on {table_name}"),
            pg_error: e,
        })?;

        // Install statement-level AFTER trigger (flushes the refresh queue).
        // This ensures auto-commit transactions get TVIEWs refreshed.
        // For explicit transactions, the ProcessUtility hook also flushes on COMMIT.
        let flush_trigger_name = format!("trg_tview_flush_{tview_entity}_on_{table_name}");
        let qi_flush_trigger = quote_identifier(&flush_trigger_name);
        if !trigger_exists(&table_name, &flush_trigger_name)? {
            let flush_sql = format!(
                "CREATE TRIGGER {qi_flush_trigger}
                 AFTER INSERT OR UPDATE OR DELETE ON {qi_table}
                 FOR EACH STATEMENT
                 EXECUTE FUNCTION pg_tview_flush_trigger()"
            );
            crate::utils::spi_run_ddl(&flush_sql).map_err(|e| TViewError::CatalogError {
                operation: format!("Install flush trigger on {table_name}"),
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
pub fn remove_triggers(
    table_oids: &[pg_sys::Oid],
    tview_entity: &str,
) -> TViewResult<()> {
    for &table_oid in table_oids {
        let table_name = get_table_name(table_oid)?;
        let qi_table = quote_identifier(&table_name);
        let trigger_name = format!("trg_tview_{tview_entity}_on_{table_name}");
        let flush_trigger_name = format!("trg_tview_flush_{tview_entity}_on_{table_name}");

        let drop_sql = format!(
            "DROP TRIGGER IF EXISTS {} ON {qi_table}",
            quote_identifier(&trigger_name),
        );
        crate::utils::spi_run_ddl(&drop_sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Drop trigger from {table_name}"),
            pg_error: e,
        })?;

        let drop_flush_sql = format!(
            "DROP TRIGGER IF EXISTS {} ON {qi_table}",
            quote_identifier(&flush_trigger_name),
        );
        crate::utils::spi_run_ddl(&drop_flush_sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Drop flush trigger from {table_name}"),
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
            let entity: String = row["entity"]
                .value()?
                .ok_or_else(|| spi::Error::from(TViewError::SpiError {
                    query: "migrate: SELECT entity".to_string(),
                    error: "entity column is NULL".to_string(),
                }))?;
            let table_oid: pg_sys::Oid = row["table_oid"]
                .value()?
                .ok_or_else(|| spi::Error::from(TViewError::SpiError {
                    query: "migrate: SELECT table_oid".to_string(),
                    error: "table_oid column is NULL".to_string(),
                }))?;
            out.push((entity, table_oid));
        }
        Ok(out)
    })
    .map_err(|e: spi::Error| TViewError::CatalogError {
        operation: "Migrate triggers: read pg_tview_meta".to_string(),
        pg_error: format!("{e:?}"),
    })?;

    for (entity, table_oid) in pairs {
        let table_name = get_table_name(table_oid)?;
        let qi_table = quote_identifier(&table_name);
        let trigger_name = format!("trg_tview_{entity}_on_{table_name}");
        let qi_trigger = quote_identifier(&trigger_name);

        // Drop the old trigger (IF EXISTS makes this safe if already removed)
        let drop_sql = format!("DROP TRIGGER IF EXISTS {qi_trigger} ON {qi_table}");
        crate::utils::spi_run_ddl(&drop_sql).map_err(|e| TViewError::CatalogError {
            operation: format!("Migrate trigger: drop {trigger_name} on {table_name}"),
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
            operation: format!("Migrate trigger: create {trigger_name} on {table_name}"),
            pg_error: e,
        })?;

        // Install statement-level flush trigger
        let flush_trigger_name = format!("trg_tview_flush_{entity}_on_{table_name}");
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

fn get_table_name(oid: pg_sys::Oid) -> TViewResult<String> {
    crate::utils::spi_get_string(&format!(
        "SELECT relname::text FROM pg_class WHERE oid = {oid:?}"
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get table name for OID {oid:?}"),
        pg_error: format!("{e:?}"),
    })?
    .ok_or_else(|| TViewError::DependencyResolutionFailed {
        view_name: format!("OID {oid:?}"),
        reason: "Table not found".to_string(),
    })
}

fn trigger_exists(table_name: &str, trigger_name: &str) -> TViewResult<bool> {
    let args = vec![
        unsafe { DatumWithOid::new(table_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        unsafe { DatumWithOid::new(trigger_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
    ];
    Spi::get_one_with_args::<bool>(
        "SELECT COUNT(*) > 0 FROM pg_trigger
         WHERE tgrelid = $1::regclass
           AND tgname = $2",
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check trigger {trigger_name}"),
        pg_error: format!("{e:?}"),
    })
    .map(|opt| opt.unwrap_or(false))
}

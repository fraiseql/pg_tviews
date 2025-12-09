use pgrx::prelude::*;
use crate::error::{TViewError, TViewResult};

pub fn install_triggers(
    table_oids: &[pg_sys::Oid],
    tview_entity: &str,
) -> TViewResult<()> {
    // First, create trigger handler function if not exists
    create_trigger_handler()?;

    // Install trigger on each base table
    for &table_oid in table_oids {
        let table_name = get_table_name(table_oid)?;

        // Use deterministic trigger name: trg_tview_{entity}_on_{table}
        let trigger_name = format!("trg_tview_{}_on_{}", tview_entity, table_name);

        // Check if trigger already exists
        if trigger_exists(&table_name, &trigger_name)? {
            warning!("Trigger {} already exists on {}, skipping", trigger_name, table_name);
            continue;
        }

        // Install AFTER INSERT OR UPDATE OR DELETE trigger
        // Pass entity name as trigger argument
        let trigger_sql = format!(
            "CREATE TRIGGER {}
             AFTER INSERT OR UPDATE OR DELETE ON {}
             FOR EACH ROW
             EXECUTE FUNCTION tview_trigger_handler('{}')",
            trigger_name, table_name, tview_entity
        );

        Spi::run(&trigger_sql)
            .map_err(|e| TViewError::CatalogError {
                operation: format!("Install trigger on {}", table_name),
                pg_error: format!("{:?}", e),
            })?;

        info!("Installed trigger {} on {}", trigger_name, table_name);
    }

    Ok(())
}

pub fn remove_triggers(
    table_oids: &[pg_sys::Oid],
    tview_entity: &str,
) -> TViewResult<()> {
    for &table_oid in table_oids {
        let table_name = get_table_name(table_oid)?;
        let trigger_name = format!("trg_tview_{}_on_{}", tview_entity, table_name);

        let drop_sql = format!(
            "DROP TRIGGER IF EXISTS {} ON {}",
            trigger_name, table_name
        );

        Spi::run(&drop_sql)
            .map_err(|e| TViewError::CatalogError {
                operation: format!("Drop trigger from {}", table_name),
                pg_error: format!("{:?}", e),
            })?;

        info!("Removed trigger {} from {}", trigger_name, table_name);
    }

    Ok(())
}

fn create_trigger_handler() -> TViewResult<()> {
    // Check if extension jsonb_ivm is installed
    let has_jsonb_ivm = Spi::get_one::<bool>(
        "SELECT COUNT(*) > 0 FROM pg_extension WHERE extname = 'jsonb_ivm'"
    )
    .map_err(|e| TViewError::CatalogError {
        operation: "Check jsonb_ivm extension".to_string(),
        pg_error: format!("{:?}", e),
    })?
    .unwrap_or(false);

    if !has_jsonb_ivm {
        return Err(TViewError::JsonbIvmNotInstalled);
    }

    let handler_sql = r#"
        CREATE OR REPLACE FUNCTION tview_trigger_handler()
        RETURNS TRIGGER AS $$
        BEGIN
            -- For now, just log that trigger fired
            -- Actual refresh logic will be in Phase 4
            RAISE NOTICE 'TVIEW trigger fired on table % for operation %',
                TG_TABLE_NAME, TG_OP;

            -- Return appropriate value based on operation
            IF TG_OP = 'DELETE' THEN
                RETURN OLD;
            ELSE
                RETURN NEW;
            END IF;
        END;
        $$ LANGUAGE plpgsql;
    "#;

    Spi::run(handler_sql)
        .map_err(|e| TViewError::CatalogError {
            operation: "Create trigger handler".to_string(),
            pg_error: format!("{:?}", e),
        })?;

    Ok(())
}

fn get_table_name(oid: pg_sys::Oid) -> TViewResult<String> {
    Spi::get_one::<String>(&format!(
        "SELECT relname FROM pg_class WHERE oid = {:?}",
        oid
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get table name for OID {:?}", oid),
        pg_error: format!("{:?}", e),
    })?
    .ok_or_else(|| TViewError::DependencyResolutionFailed {
        view_name: format!("OID {:?}", oid),
        reason: "Table not found".to_string(),
    })
}

fn trigger_exists(table_name: &str, trigger_name: &str) -> TViewResult<bool> {
    Spi::get_one::<bool>(&format!(
        "SELECT COUNT(*) > 0 FROM pg_trigger
         WHERE tgrelid = '{}'::regclass
           AND tgname = '{}'",
        table_name, trigger_name
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check trigger {}", trigger_name),
        pg_error: format!("{:?}", e),
    })
    .map(|opt| opt.unwrap_or(false))
}

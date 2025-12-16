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
        let trigger_name = format!("trg_tview_{tview_entity}_on_{table_name}");

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
                operation: format!("Install trigger on {table_name}"),
                pg_error: format!("{e:?}"),
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
        let trigger_name = format!("trg_tview_{tview_entity}_on_{table_name}");

        let drop_sql = format!(
            "DROP TRIGGER IF EXISTS {} ON {}",
            trigger_name, table_name
        );

        Spi::run(&drop_sql)
            .map_err(|e| TViewError::CatalogError {
                operation: format!("Drop trigger from {table_name}"),
                pg_error: format!("{e:?}"),
            })?;

        info!("Removed trigger {} from {}", trigger_name, table_name);
    }

    Ok(())
}

fn create_trigger_handler() -> TViewResult<()> {
    // Note: jsonb_ivm dependency was removed - we don't use it anymore
    // Triggers are installed directly without needing external extensions

    let handler_sql = r"
        CREATE OR REPLACE FUNCTION tview_trigger_handler()
        RETURNS TRIGGER AS $$
        DECLARE
            pk_col_name TEXT;
            pk_val_old BIGINT;
            pk_val_new BIGINT;
            entity_name TEXT;
        BEGIN
            -- Get PK column name for the changed table dynamically
            SELECT a.attname INTO pk_col_name
            FROM pg_index i
            JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
            WHERE i.indrelid = TG_RELID AND i.indisprimary
            LIMIT 1;

            IF pk_col_name IS NULL THEN
                RAISE EXCEPTION 'Table % has no primary key', TG_TABLE_NAME;
            END IF;

            -- Extract PK values dynamically based on operation
            IF TG_OP = 'DELETE' OR TG_OP = 'UPDATE' THEN
                EXECUTE format('SELECT ($1).%I', pk_col_name) USING OLD INTO pk_val_old;
            END IF;

            IF TG_OP = 'INSERT' OR TG_OP = 'UPDATE' THEN
                EXECUTE format('SELECT ($1).%I', pk_col_name) USING NEW INTO pk_val_new;
            END IF;

            -- Log the trigger action
            RAISE NOTICE 'TVIEW trigger fired: table=%, op=%, pk_col=%, old_pk=%, new_pk=%',
                TG_TABLE_NAME, TG_OP, pk_col_name, pk_val_old, pk_val_new;

            -- Handle different operations appropriately
            IF TG_OP = 'INSERT' THEN
                -- For INSERT: Check if this contributes to array elements
                PERFORM pg_tviews_insert(TG_RELID, pk_val_new);
            ELSIF TG_OP = 'UPDATE' THEN
                -- For UPDATE: Use existing cascade logic for smart patching
                PERFORM pg_tviews_cascade(TG_RELID, pk_val_new);
            ELSIF TG_OP = 'DELETE' THEN
                -- For DELETE: Remove from array elements
                PERFORM pg_tviews_delete(TG_RELID, pk_val_old);
            END IF;

            -- Return appropriate value based on operation
            IF TG_OP = 'DELETE' THEN
                RETURN OLD;
            ELSE
                RETURN NEW;
            END IF;
        END;
        $$ LANGUAGE plpgsql;
    ";

    Spi::run(handler_sql)
        .map_err(|e| TViewError::CatalogError {
            operation: "Create trigger handler".to_string(),
            pg_error: format!("{e:?}"),
        })?;

    Ok(())
}

fn get_table_name(oid: pg_sys::Oid) -> TViewResult<String> {
    Spi::get_one::<String>(&format!(
        "SELECT relname::text FROM pg_class WHERE oid = {:?}",
        oid
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
    Spi::get_one::<bool>(&format!(
        "SELECT COUNT(*) > 0 FROM pg_trigger
         WHERE tgrelid = '{}'::regclass
           AND tgname = '{}'",
        table_name, trigger_name
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check trigger {trigger_name}"),
        pg_error: format!("{e:?}"),
    })
    .map(|opt| opt.unwrap_or(false))
}

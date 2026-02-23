use pgrx::prelude::*;

/// Log TVIEW creation
pub fn log_create(entity: &str, definition: &str) -> spi::Result<()> {
    let current_user = crate::utils::spi_get_string("SELECT current_user")?
        .unwrap_or_else(|| "unknown".to_string());

    Spi::run(&format!(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('CREATE', '{}', '{}', '{}'::jsonb)",
        entity.replace('\'', "''"),
        current_user.replace('\'', "''"),
        serde_json::json!({
            "definition": definition,
            "version": env!("CARGO_PKG_VERSION")
        })
    ))?;

    Ok(())
}

/// Log TVIEW drop
pub fn log_drop(entity: &str) -> spi::Result<()> {
    let current_user = crate::utils::spi_get_string("SELECT current_user")?
        .unwrap_or_else(|| "unknown".to_string());

    Spi::run(&format!(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('DROP', '{}', '{}', '{{}}'::jsonb)",
        entity.replace('\'', "''"),
        current_user.replace('\'', "''")
    ))?;

    Ok(())
}

/// Log TVIEW refresh operation
#[allow(dead_code)]
pub fn log_refresh(entity: &str, rows_affected: i64) -> spi::Result<()> {
    let current_user = crate::utils::spi_get_string("SELECT current_user")?
        .unwrap_or_else(|| "unknown".to_string());

    Spi::run(&format!(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('REFRESH', '{}', '{}', '{}'::jsonb)",
        entity.replace('\'', "''"),
        current_user.replace('\'', "''"),
        serde_json::json!({
            "rows_affected": rows_affected
        })
    ))?;

    Ok(())
}
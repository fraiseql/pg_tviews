use pgrx::prelude::*;
use pgrx::datum::DatumWithOid;

/// Log TVIEW creation
pub fn log_create(entity: &str, definition: &str) -> spi::Result<()> {
    let current_user = crate::utils::spi_get_string("SELECT current_user")?
        .unwrap_or_else(|| "unknown".to_string());

    let json_str = serde_json::json!({
        "definition": definition,
        "version": env!("CARGO_PKG_VERSION")
    }).to_string();

    Spi::run_with_args(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('CREATE', $1, $2, $3::jsonb)",
        &[
            unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(current_user.as_str(), PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(json_str.as_str(), PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        ],
    )?;

    Ok(())
}

/// Log TVIEW drop
pub fn log_drop(entity: &str) -> spi::Result<()> {
    let current_user = crate::utils::spi_get_string("SELECT current_user")?
        .unwrap_or_else(|| "unknown".to_string());

    Spi::run_with_args(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('DROP', $1, $2, '{}'::jsonb)",
        &[
            unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(current_user.as_str(), PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        ],
    )?;

    Ok(())
}

/// Log TVIEW refresh operation
#[allow(dead_code)] // Reason: audit logging — will be wired to refresh paths
pub fn log_refresh(entity: &str, rows_affected: i64) -> spi::Result<()> {
    let current_user = crate::utils::spi_get_string("SELECT current_user")?
        .unwrap_or_else(|| "unknown".to_string());

    let json_str = serde_json::json!({ "rows_affected": rows_affected }).to_string();

    Spi::run_with_args(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('REFRESH', $1, $2, $3::jsonb)",
        &[
            unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(current_user.as_str(), PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(json_str.as_str(), PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        ],
    )?;

    Ok(())
}

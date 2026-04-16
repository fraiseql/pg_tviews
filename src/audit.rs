use pgrx::prelude::*;
use pgrx::datum::DatumWithOid;

/// Return the authenticated session user (not affected by SET ROLE).
fn session_user() -> spi::Result<String> {
    Ok(crate::utils::spi_get_string("SELECT session_user::text")?
        .unwrap_or_else(|| "unknown".to_string()))
}

/// Log TVIEW creation
pub fn log_create(entity: &str, definition: &str) -> spi::Result<()> {
    let user = session_user()?;

    let json_str = serde_json::json!({
        "definition": definition,
        "version": env!("CARGO_PKG_VERSION")
    }).to_string();

    let user_ref: &str = &user;
    let json_ref: &str = &json_str;
    Spi::run_with_args(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('CREATE', $1, $2, $3::jsonb)",
        &[
            unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(user_ref, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(json_ref, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        ],
    )?;

    Ok(())
}

/// Log TVIEW drop
pub fn log_drop(entity: &str) -> spi::Result<()> {
    let user = session_user()?;

    let user_ref: &str = &user;
    Spi::run_with_args(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('DROP', $1, $2, '{}'::jsonb)",
        &[
            unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(user_ref, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        ],
    )?;

    Ok(())
}

/// Log TVIEW refresh operation
#[allow(dead_code)] // Reason: audit logging — will be wired to refresh paths
pub fn log_refresh(entity: &str, rows_affected: i64) -> spi::Result<()> {
    let user = session_user()?;

    let json_str = serde_json::json!({ "rows_affected": rows_affected }).to_string();

    let user_ref: &str = &user;
    let json_ref: &str = &json_str;
    Spi::run_with_args(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('REFRESH', $1, $2, $3::jsonb)",
        &[
            unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(user_ref, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(json_ref, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        ],
    )?;

    Ok(())
}

use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use std::cell::RefCell;

/// Return the authenticated session user (not affected by SET ROLE).
fn session_user() -> spi::Result<String> {
    Ok(crate::utils::spi_get_string("SELECT session_user::text")?
        .unwrap_or_else(|| "unknown".to_string()))
}

// ── Audit entry types ───────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum AuditOperation {
    Create,
    Drop,
    Refresh,
}

impl AuditOperation {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "CREATE",
            Self::Drop => "DROP",
            Self::Refresh => "REFRESH",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub entity: String,
    pub operation: AuditOperation,
    pub rows_affected: Option<i64>,
    pub details: serde_json::Value,
}

// ── Thread-local transaction buffer ─────────────────────────────────────

thread_local! {
    static AUDIT_BUFFER: RefCell<Vec<AuditEntry>> = const { RefCell::new(Vec::new()) };
}

/// Push an audit entry into the transaction buffer. No SPI, no GUC check.
fn buffer_entry(entry: AuditEntry) {
    AUDIT_BUFFER.with(|buf| buf.borrow_mut().push(entry));
}

/// Buffer a TVIEW creation event.
pub fn log_create(entity: &str, definition: &str) {
    buffer_entry(AuditEntry {
        entity: entity.to_string(),
        operation: AuditOperation::Create,
        rows_affected: None,
        details: serde_json::json!({
            "definition": definition,
            "version": env!("CARGO_PKG_VERSION"),
        }),
    });
}

/// Buffer a TVIEW drop event.
pub fn log_drop(entity: &str) {
    buffer_entry(AuditEntry {
        entity: entity.to_string(),
        operation: AuditOperation::Drop,
        rows_affected: None,
        details: serde_json::Value::Null,
    });
}

/// Buffer a TVIEW refresh event.
pub fn log_refresh(entity: &str, rows_affected: i64) {
    buffer_entry(AuditEntry {
        entity: entity.to_string(),
        operation: AuditOperation::Refresh,
        rows_affected: Some(rows_affected),
        details: serde_json::Value::Null,
    });
}

/// Flush all buffered audit entries via a single bulk INSERT.
///
/// Gated by `pg_tviews.audit_enabled` — if disabled, just clears the buffer.
///
/// **MUST be called from ProcessUtility hook COMMIT path** (where SPI is safe).
/// MUST NOT be called from xact callbacks.
pub fn flush_audit_buffer() -> spi::Result<()> {
    let entries: Vec<AuditEntry> =
        AUDIT_BUFFER.with(|buf| buf.borrow_mut().drain(..).collect());

    if entries.is_empty() || !crate::config::audit_enabled() {
        return Ok(());
    }

    let user = session_user()?;

    // Serialize all entries as a JSON array and unpack server-side.
    // This avoids delimiter-collision issues with string_to_array.
    let json_array: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "op": e.operation.as_str(),
                "entity": e.entity,
                "rows": e.rows_affected,
                "details": e.details,
            })
        })
        .collect();
    let payload = serde_json::Value::Array(json_array).to_string();

    let user_ref: &str = &user;
    let payload_ref: &str = &payload;

    Spi::run_with_args(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, rows_affected, details)
         SELECT
             e->>'op',
             e->>'entity',
             $2,
             (e->>'rows')::bigint,
             CASE WHEN e->'details' = 'null'::jsonb THEN NULL ELSE e->'details' END
         FROM jsonb_array_elements($1::jsonb) AS e",
        &[
            unsafe {
                DatumWithOid::new(payload_ref, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
            },
            unsafe {
                DatumWithOid::new(user_ref, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
            },
        ],
    )?;

    Ok(())
}

/// Clear the audit buffer without flushing. Safe to call from xact ABORT
/// callbacks (no SPI needed — just drops the Vec contents).
pub fn clear_audit_buffer() {
    AUDIT_BUFFER.with(|buf| buf.borrow_mut().clear());
}

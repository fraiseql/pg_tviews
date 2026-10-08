//! Input Validation Module
//!
//! Provides security-critical validation functions to prevent SQL injection
//! and other input-based attacks at system boundaries.

use crate::error::{TViewError, TViewResult};

/// Validate `PostgreSQL` identifier (table, column, schema names)
///
/// Allows: alphanumeric + underscore. Rejects: quotes, semicolons, dashes,
/// spaces, special chars, SQL keywords, identifiers starting with digits,
/// and identifiers exceeding 63 characters.
pub fn validate_sql_identifier(identifier: &str, param_name: &str) -> TViewResult<()> {
    if identifier.is_empty() {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            reason: "Identifier cannot be empty".to_string(),
        });
    }

    // Ensure valid identifier characters (alphanumeric + underscore)
    if !identifier.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            reason: format!(
                "Identifier '{}' contains invalid characters (only alphanumeric and underscore allowed)",
                sanitize_for_logging(identifier)
            ),
        });
    }

    // PostgreSQL identifiers can't start with digit (unless quoted)
    if identifier.starts_with(|c: char| c.is_numeric()) {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            reason: "Identifier cannot start with a digit".to_string(),
        });
    }

    // Length limit (PostgreSQL max identifier length is 63)
    if identifier.len() > 63 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            reason: format!("Identifier too long ({} chars, max 63)", identifier.len()),
        });
    }

    Ok(())
}

/// Sanitize string for logging (truncate, remove control chars)
fn sanitize_for_logging(s: &str) -> String {
    let cut = crate::utils::truncate_chars(s, 50);
    let truncated = if cut.len() < s.len() {
        format!("{cut}...")
    } else {
        s.to_string()
    };

    truncated
        .replace('\0', "\\0")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_identifiers() {
        assert!(validate_sql_identifier("my_table", "test").is_ok());
        assert!(validate_sql_identifier("user_data", "test").is_ok());
        assert!(validate_sql_identifier("pk_user", "test").is_ok());
        assert!(validate_sql_identifier("table123", "test").is_ok());
    }

    #[test]
    fn test_invalid_identifiers() {
        assert!(validate_sql_identifier("", "test").is_err());
        assert!(validate_sql_identifier("table; DROP", "test").is_err());
        assert!(validate_sql_identifier("user-data", "test").is_err());
        assert!(validate_sql_identifier("my table", "test").is_err());
        assert!(validate_sql_identifier("'admin'", "test").is_err());
        assert!(validate_sql_identifier("123table", "test").is_err());
    }

    #[test]
    fn test_rejected_multibyte_identifier_reports_without_panicking() {
        // 'a' + 30 two-byte chars + ';' puts a char boundary mid-way through byte 50.
        let name = format!("a{};", "é".repeat(30));
        let err = validate_sql_identifier(&name, "test").unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }
}

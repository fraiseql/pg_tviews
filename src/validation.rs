//! Input Validation Module
//!
//! This module provides security-critical validation functions used throughout
//! `pg_tviews` to prevent SQL injection and other input-based attacks.
//!
//! ## Security Principles
//!
//! 1. **Whitelist, not blacklist**: Only allow known-safe characters
//! 2. **Validate early**: Check inputs before any processing
//! 3. **Fail securely**: Return clear errors on invalid input
//! 4. **No exceptions**: Every external input must be validated
//!
//! ## Usage
//!
//! ```rust
//! use crate::validation::{validate_sql_identifier, validate_jsonb_path};
//!
//! // Validate before using in SQL
//! validate_sql_identifier(table_name, "table_name")?;
//! let sql = format!("SELECT * FROM {}", table_name); // Now safe
//! ```

use crate::error::{TViewError, TViewResult};

/// Validate `PostgreSQL` identifier (table, column, schema names)
///
/// # Security
///
/// Prevents SQL injection by ensuring only safe identifier characters.
/// Allows: alphanumeric + underscore (`PostgreSQL` identifier rules)
/// Rejects: quotes, semicolons, dashes, spaces, special chars
///
/// # Arguments
///
/// * `identifier` - String to validate
/// * `param_name` - Parameter name for error messages
///
/// # Returns
///
/// `Ok(())` if valid, `Err` with descriptive message if invalid
///
/// # Examples
///
/// ```rust
/// // Valid identifiers
/// validate_sql_identifier("my_table", "table_name")?;     // ✓
/// validate_sql_identifier("user_data", "column")?;         // ✓
/// validate_sql_identifier("pk_user", "pk_column")?;        // ✓
///
/// // Invalid identifiers
/// validate_sql_identifier("users; DROP TABLE", "table")?;  // ✗ SQL injection
/// validate_sql_identifier("user-data", "table")?;          // ✗ Contains dash
/// validate_sql_identifier("my table", "table")?;           // ✗ Contains space
/// validate_sql_identifier("'admin'", "column")?;           // ✗ Contains quotes
/// ```
pub fn validate_sql_identifier(identifier: &str, param_name: &str) -> TViewResult<()> {
    // Check for empty
    if identifier.is_empty() {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: identifier.to_string(),
            reason: "Identifier cannot be empty".to_string(),
        });
    }

    // Check for SQL injection patterns
    let dangerous_chars = [';', '-', '\'', '"', '/', '*', '\\', '\0'];
    for &ch in &dangerous_chars {
        if identifier.contains(ch) {
            return Err(TViewError::SecurityViolation {
                parameter: param_name.to_string(),
                value: sanitize_for_logging(identifier),
                reason: format!("Identifier contains dangerous character: '{}'", ch),
            });
        }
    }

    // Check for SQL keywords used maliciously
    let lower = identifier.to_lowercase();
    if lower.contains("drop ") || lower.contains("delete ") ||
       lower.contains("insert ") || lower.contains("update ") ||
       lower.contains("create ") || lower.contains("alter ") {
        return Err(TViewError::SecurityViolation {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(identifier),
            reason: "Identifier contains SQL keywords".to_string(),
        });
    }

    // Ensure valid identifier characters (alphanumeric + underscore)
    if !identifier.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(identifier),
            reason: "Identifier must contain only alphanumeric characters and underscores".to_string(),
        });
    }

    // PostgreSQL identifiers can't start with digit (unless quoted)
    if identifier.chars().next().unwrap().is_numeric() {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(identifier),
            reason: "Identifier cannot start with a digit".to_string(),
        });
    }

    // Length limit (PostgreSQL max identifier length is 63)
    if identifier.len() > 63 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: format!("{}... ({} chars)", &identifier[..20], identifier.len()),
            reason: "Identifier too long (max 63 characters)".to_string(),
        });
    }

    Ok(())
}

/// Validate JSONB path syntax (dot notation + array indices)
///
/// # Security
///
/// Prevents injection while allowing complex path navigation.
/// Allows: alphanumeric, dots, brackets, underscores
/// Rejects: quotes, semicolons, SQL keywords, mismatched brackets
///
/// # Path Syntax
///
/// - Object navigation: `field.subfield.deep`
/// - Array indexing: `items[0]` or `items[123]`
/// - Combined: `users[0].profile.settings.theme`
///
/// # Constraints
///
/// - Maximum depth: 100 segments
/// - Maximum length: 500 characters
/// - Brackets must be matched
/// - Array indices must be non-negative integers
/// - No spaces or special characters
///
/// # Examples
///
/// ```rust
/// // Valid paths
/// validate_jsonb_path("author.name", "path")?;                    // ✓
/// validate_jsonb_path("items[0]", "path")?;                       // ✓
/// validate_jsonb_path("users[5].profile.email", "path")?;         // ✓
/// validate_jsonb_path("metadata.tags[0].value", "path")?;         // ✓
///
/// // Invalid paths
/// validate_jsonb_path("field'; DROP TABLE", "path")?;             // ✗ Injection
/// validate_jsonb_path("items[", "path")?;                         // ✗ Unmatched bracket
/// validate_jsonb_path("items[-1]", "path")?;                       // ✗ Negative index
/// validate_jsonb_path("author's.name", "path")?;                  // ✗ Apostrophe
/// ```
pub fn validate_jsonb_path(path: &str, param_name: &str) -> TViewResult<()> {
    // Check for empty
    if path.is_empty() {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: path.to_string(),
            reason: "Path cannot be empty".to_string(),
        });
    }

    // Length limit
    if path.len() > 500 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: format!("{}... ({} chars)", &path[..50], path.len()),
            reason: "Path too long (max 500 characters)".to_string(),
        });
    }

    // Check for SQL injection patterns
    if path.contains(';') || path.contains("--") || path.contains("/*") ||
       path.contains('\'') || path.contains('"') {
        return Err(TViewError::SecurityViolation {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(path),
            reason: "Path contains SQL injection patterns".to_string(),
        });
    }

    // Validate allowed characters
    let valid = path.chars().all(|c| {
        c.is_alphanumeric() || c == '.' || c == '[' || c == ']' || c == '_'
    });

    if !valid {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(path),
            reason: "Path contains invalid characters (allowed: alphanumeric, dots, brackets, underscore)".to_string(),
        });
    }

    // Validate bracket matching and array indices
    validate_bracket_matching(path, param_name)?;
    validate_array_indices(path, param_name)?;

    // Validate depth (max 100 levels)
    let depth = path.split('.').count() + path.matches('[').count();
    if depth > 100 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: format!("depth={}", depth),
            reason: "Path too deep (max 100 levels)".to_string(),
        });
    }

    Ok(())
}

/// Validate bracket matching in paths
fn validate_bracket_matching(path: &str, param_name: &str) -> TViewResult<()> {
    let mut depth = 0;

    for (pos, ch) in path.chars().enumerate() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return Err(TViewError::InvalidInput {
                        parameter: param_name.to_string(),
                        value: sanitize_for_logging(path),
                        reason: format!("Unmatched closing bracket ']' at position {}", pos),
                    });
                }
            }
            _ => {}
        }
    }

    if depth > 0 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(path),
            reason: format!("Unmatched opening bracket '[' ({} unclosed)", depth),
        });
    }

    Ok(())
}

/// Validate array indices are non-negative integers
fn validate_array_indices(path: &str, param_name: &str) -> TViewResult<()> {
    // Extract content between brackets
    let mut in_brackets = false;
    let mut current_index = String::new();

    for (pos, ch) in path.chars().enumerate() {
        match ch {
            '[' => {
                in_brackets = true;
                current_index.clear();
            }
            ']' => {
                if in_brackets && !current_index.is_empty() {
                    // Validate index is non-negative integer
                    match current_index.parse::<u32>() {
                        Ok(_) => {}
                        Err(_) => {
                            return Err(TViewError::InvalidInput {
                                parameter: param_name.to_string(),
                                value: sanitize_for_logging(path),
                                reason: format!(
                                    "Invalid array index '{}' at position {} (must be non-negative integer)",
                                    current_index, pos
                                ),
                            });
                        }
                    }
                }
                in_brackets = false;
            }
            _ if in_brackets => {
                current_index.push(ch);
            }
            _ => {}
        }
    }

    Ok(())
}

/// Sanitize string for logging (truncate, remove sensitive chars)
fn sanitize_for_logging(s: &str) -> String {
    let max_len = 50;
    let truncated = if s.len() > max_len {
        format!("{}...", &s[..max_len])
    } else {
        s.to_string()
    };

    // Remove potential sensitive characters for logging
    truncated
        .replace('\0', "\\0")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Validate table name (stricter than generic identifier)
pub fn validate_table_name(name: &str) -> TViewResult<()> {
    validate_sql_identifier(name, "table_name")?;

    // Additional table-specific validation
    if !name.starts_with("tv_") && !name.starts_with("tb_") && !name.starts_with("test_") {
        return Err(TViewError::InvalidInput {
            parameter: "table_name".to_string(),
            value: sanitize_for_logging(name),
            reason: "Table name should start with tv_, tb_, or test_ prefix".to_string(),
        });
    }

    Ok(())
}

/// Validate column name (alias for identifier)
pub fn validate_column_name(name: &str) -> TViewResult<()> {
    validate_sql_identifier(name, "column_name")
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
    fn test_valid_paths() {
        assert!(validate_jsonb_path("author.name", "test").is_ok());
        assert!(validate_jsonb_path("items[0]", "test").is_ok());
        assert!(validate_jsonb_path("users[5].profile.email", "test").is_ok());
        assert!(validate_jsonb_path("metadata.tags[0].value", "test").is_ok());
    }

    #[test]
    fn test_invalid_paths() {
        assert!(validate_jsonb_path("", "test").is_err());
        assert!(validate_jsonb_path("field'; DROP", "test").is_err());
        assert!(validate_jsonb_path("items[", "test").is_err());
        assert!(validate_jsonb_path("items]", "test").is_err());
        assert!(validate_jsonb_path("items[-1]", "test").is_err());
        assert!(validate_jsonb_path("author's.name", "test").is_err());
    }
}
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

/// Validate JSONB path syntax (dot notation + array indices)
///
/// Allows: alphanumeric, dots, brackets, underscores.
/// Rejects: quotes, semicolons, SQL patterns, mismatched brackets.
pub fn validate_jsonb_path(path: &str, param_name: &str) -> TViewResult<()> {
    if path.is_empty() {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            reason: "Path cannot be empty".to_string(),
        });
    }

    if path.len() > 500 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            reason: format!("Path too long ({} chars, max 500)", path.len()),
        });
    }

    // Validate allowed characters
    if !path
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '.' | '[' | ']' | '_'))
    {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            reason: format!(
                "Path '{}' contains invalid characters (allowed: alphanumeric, dots, brackets, underscore)",
                sanitize_for_logging(path)
            ),
        });
    }

    validate_bracket_matching(path, param_name)?;
    validate_array_indices(path, param_name)?;

    // Validate depth (max 100 levels)
    let depth = path.split('.').count() + path.matches('[').count();
    if depth > 100 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            reason: format!("Path too deep (depth={depth}, max 100)"),
        });
    }

    Ok(())
}

/// Validate bracket matching in paths
fn validate_bracket_matching(path: &str, param_name: &str) -> TViewResult<()> {
    let mut depth: i32 = 0;

    for (pos, ch) in path.chars().enumerate() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return Err(TViewError::InvalidInput {
                        parameter: param_name.to_string(),
                        reason: format!("Unmatched closing bracket ']' at position {pos}"),
                    });
                }
            }
            _ => {}
        }
    }

    if depth > 0 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            reason: format!("Unmatched opening bracket '[' ({depth} unclosed)"),
        });
    }

    Ok(())
}

/// Validate array indices are non-negative integers
fn validate_array_indices(path: &str, param_name: &str) -> TViewResult<()> {
    let mut in_brackets = false;
    let mut current_index = String::new();

    for (pos, ch) in path.chars().enumerate() {
        match ch {
            '[' => {
                in_brackets = true;
                current_index.clear();
            }
            ']' => {
                if in_brackets && !current_index.is_empty() && current_index.parse::<u32>().is_err()
                {
                    return Err(TViewError::InvalidInput {
                        parameter: param_name.to_string(),
                        reason: format!(
                            "Invalid array index '{current_index}' at position {pos} (must be non-negative integer)",
                        ),
                    });
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

/// Sanitize string for logging (truncate, remove control chars)
fn sanitize_for_logging(s: &str) -> String {
    let max_len = 50;
    let truncated = if s.len() > max_len {
        format!("{}...", &s[..max_len])
    } else {
        s.to_string()
    };

    truncated
        .replace('\0', "\\0")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Validate table name (stricter than generic identifier)
pub fn validate_table_name(name: &str) -> TViewResult<()> {
    validate_sql_identifier(name, "table_name")?;

    if !name.starts_with("tv_") && !name.starts_with("tb_") && !name.starts_with("test_") {
        return Err(TViewError::InvalidInput {
            parameter: "table_name".to_string(),
            reason: format!(
                "Table name '{}' should start with tv_, tb_, or test_ prefix",
                sanitize_for_logging(name)
            ),
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
        assert!(validate_jsonb_path("items[", "test").is_err());
        assert!(validate_jsonb_path("items]", "test").is_err());
    }
}

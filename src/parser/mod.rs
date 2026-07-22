//! SQL Parser: TVIEW DDL Statement Parsing
//!
//! This module parses TVIEW Data Definition Language statements:
//! - **CREATE TABLE tv_ AS SELECT**: Extracts name and SELECT statement
//! - **Validation**: Ensures proper TVIEW naming conventions
//!
//! ## Supported Syntax
//!
//! ```sql
//! -- Create TVIEW
//! CREATE TABLE tv_entity AS SELECT id, data FROM base_table;
//!
//! -- Drop TVIEW (handled by ProcessUtility hook, not parser)
//! DROP TABLE tv_entity;
//!
//! -- Schema-qualified
//! CREATE TABLE schema.tv_entity AS SELECT * FROM schema.base_table;
//! ```
//!
//! ## Note on DROP Syntax
//!
//! `DROP TABLE tv_*` is handled directly by the `ProcessUtility` hook in `src/hooks.rs`,
//! not by this parser module. The hook intercepts `DROP TABLE` statements and checks
//! if the table name starts with `tv_`, then calls the `drop_tview()` function.
//!
//! ## Limitations (v1)
//!
//! - Regex-based parsing (not full SQL parser) for the `CREATE TABLE tv_* AS`
//!   envelope only; the inner SELECT is analysed by the full parser in `sql_parser`
//! - `WITH RECURSIVE` is rejected at create time (see `ddl::create`)
//! - Comments may cause parsing issues
//! - String literals containing keywords may confuse parser

use crate::error::{TViewError, TViewResult};
use regex::Regex;
use std::sync::LazyLock;

/// Cached regex for parsing CREATE TABLE tv_* AS SELECT statements
static CREATE_TVIEW_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)                          # Case-insensitive, verbose
        CREATE\s+TABLE\s+                # CREATE TABLE keyword
        (?:(\w+)\.)?                     # Optional schema name
        (\w+)                            # Table name (required)
        \s+AS\s+                         # AS keyword
        (.+)                             # SELECT statement (rest of query)
        ",
    )
    .expect("CREATE_TVIEW_REGEX pattern is valid")
});

/// Get the cached CREATE TABLE regex pattern (compiled once at first use)
fn get_create_tview_regex() -> &'static Regex {
    &CREATE_TVIEW_REGEX
}

#[derive(Debug, Clone)]
pub struct CreateTViewStmt {
    pub tview_name: String,
    pub schema_name: Option<String>,
    pub select_sql: String,
}

/// Parse `CREATE TABLE tv_` AS SELECT statement
///
/// Supported syntax:
/// - `CREATE TABLE tv_name AS SELECT ...`
/// - `CREATE TABLE schema.tv_name AS SELECT ...`
///
/// Limitations (v1):
/// - No parenthesized `SELECT`
/// - Comments may cause issues
/// - String literals containing `AS` may confuse parser
///
/// # Errors
/// Returns error if SQL doesn't match CREATE TABLE AS pattern
pub fn parse_create_tview(sql: &str) -> TViewResult<CreateTViewStmt> {
    let re = get_create_tview_regex();

    let caps = re
        .captures(sql.trim())
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: sql.to_string(),
            reason: "Could not parse CREATE TABLE tv_ AS SELECT statement. \
                     Syntax: CREATE TABLE tv_name AS SELECT ...\n\
                     See docs for limitations."
                .to_string(),
        })?;

    let schema_name = caps.get(1).map(|m| m.as_str().to_string());
    let tview_name = caps
        .get(2)
        .ok_or_else(|| TViewError::InvalidTViewName {
            name: String::new(),
            reason: "Missing TVIEW name".to_string(),
        })?
        .as_str()
        .to_string();
    let select_sql = caps
        .get(3)
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: sql.to_string(),
            reason: "Missing SELECT statement after AS".to_string(),
        })?
        .as_str()
        .trim()
        .to_string();

    // Validate TVIEW name format
    if !tview_name.starts_with("tv_") {
        return Err(TViewError::InvalidTViewName {
            name: tview_name,
            reason: "TVIEW name must start with 'tv_'".to_string(),
        });
    }

    // Basic validation of SELECT statement
    if !select_sql.to_uppercase().starts_with("SELECT") {
        return Err(TViewError::InvalidSelectStatement {
            sql: select_sql,
            reason: "Expected SELECT statement after AS".to_string(),
        });
    }

    // Warn about features the regex envelope may mishandle. CTEs are handled by
    // the full parser (recursive CTEs are rejected downstream at create time), so
    // no blanket WITH warning here.
    if select_sql.contains("/*") || select_sql.contains("--") {
        pgrx::warning!("Comments in SELECT may cause parsing issues in v1");
    }

    Ok(CreateTViewStmt {
        tview_name,
        schema_name,
        select_sql,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let sql = "CREATE TABLE tv_post AS SELECT * FROM tb_post";
        let parsed = parse_create_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert!(parsed.schema_name.is_none());
        assert!(parsed.select_sql.contains("SELECT"));
    }

    #[test]
    fn test_parse_with_schema() {
        let sql = "CREATE TABLE public.tv_post AS SELECT pk_post FROM tb_post";
        let parsed = parse_create_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert_eq!(parsed.schema_name, Some("public".to_string()));
    }

    #[test]
    fn test_parse_multiline() {
        let sql = r"
            CREATE TABLE tv_post AS
            SELECT
                pk_post,
                id,
                data
            FROM tb_post
        ";
        let parsed = parse_create_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert!(parsed.select_sql.contains("pk_post"));
    }

    #[test]
    fn test_parse_invalid_name() {
        let sql = "CREATE TABLE bad_name AS SELECT * FROM tb";
        let result = parse_create_tview(sql);

        assert!(result.is_err());
        match result.unwrap_err() {
            TViewError::InvalidTViewName { name, .. } => {
                assert_eq!(name, "bad_name");
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_create_tview_regex_cached() {
        // Verify that the CREATE TABLE regex is cached and reusable
        let re = get_create_tview_regex();

        // Test that the cached regex works
        let sql = "CREATE TABLE tv_user AS SELECT id, data FROM tb_user";
        assert!(
            re.is_match(sql),
            "Cached regex should match CREATE TABLE statement"
        );

        // Verify captures work
        let caps = re.captures(sql);
        assert!(caps.is_some(), "Should capture groups from CREATE TABLE");
        let caps = caps.unwrap();
        assert_eq!(
            caps.get(2).map(|m| m.as_str()),
            Some("tv_user"),
            "Should capture table name"
        );
    }
}

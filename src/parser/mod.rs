use crate::error::{TViewError, TViewResult};
use regex::Regex;

#[derive(Debug, Clone)]
pub struct CreateTViewStmt {
    pub tview_name: String,
    pub schema_name: Option<String>,
    pub select_sql: String,
}

#[derive(Debug, Clone)]
pub struct DropTViewStmt {
    pub tview_name: String,
    pub schema_name: Option<String>,
    pub if_exists: bool,
}

/// Parse CREATE TVIEW statement
///
/// Supported syntax:
/// - CREATE TVIEW tv_name AS SELECT ...
/// - CREATE TVIEW schema.tv_name AS SELECT ...
///
/// Limitations (v1):
/// - No CTE support (WITH clause)
/// - No parenthesized SELECT
/// - Comments may cause issues
/// - String literals containing 'AS' may confuse parser
pub fn parse_create_tview(sql: &str) -> TViewResult<CreateTViewStmt> {
    let re = Regex::new(
        r"(?ix)                          # Case-insensitive, verbose
        CREATE\s+TVIEW\s+                # CREATE TVIEW keyword
        (?:(\w+)\.)?                     # Optional schema name
        (\w+)                            # Table name (required)
        \s+AS\s+                         # AS keyword
        (.+)                             # SELECT statement (rest of query)
        "
    ).map_err(|e| TViewError::InternalError {
        message: format!("Regex compilation failed: {}", e),
        file: file!(),
        line: line!(),
    })?;

    let caps = re.captures(sql.trim())
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: sql.to_string(),
            reason: "Could not parse CREATE TVIEW statement. \
                     Syntax: CREATE TVIEW name AS SELECT ...\n\
                     See docs for limitations.".to_string(),
        })?;

    let schema_name = caps.get(1).map(|m| m.as_str().to_string());
    let tview_name = caps.get(2)
        .ok_or_else(|| TViewError::InvalidTViewName {
            name: String::new(),
            reason: "Missing TVIEW name".to_string(),
        })?
        .as_str()
        .to_string();
    let select_sql = caps.get(3)
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
            name: tview_name.clone(),
            reason: "TVIEW name must start with 'tv_'".to_string(),
        });
    }

    // Basic validation of SELECT statement
    if !select_sql.to_uppercase().starts_with("SELECT") {
        return Err(TViewError::InvalidSelectStatement {
            sql: select_sql.clone(),
            reason: "Expected SELECT statement after AS".to_string(),
        });
    }

    // Warn about unsupported features
    if select_sql.to_uppercase().contains(" WITH ") {
        pgrx::warning!("CTEs (WITH clause) may not be fully supported in v1");
    }

    if select_sql.contains("/*") || select_sql.contains("--") {
        pgrx::warning!("Comments in SELECT may cause parsing issues in v1");
    }

    Ok(CreateTViewStmt {
        tview_name,
        schema_name,
        select_sql,
    })
}

/// Parse DROP TVIEW statement
pub fn parse_drop_tview(sql: &str) -> TViewResult<DropTViewStmt> {
    let re = Regex::new(
        r"(?ix)
        DROP\s+TVIEW\s+
        (IF\s+EXISTS\s+)?                # Optional IF EXISTS
        (?:(\w+)\.)?                     # Optional schema
        (\w+)                            # Table name
        "
    ).map_err(|e| TViewError::InternalError {
        message: format!("Regex compilation: {}", e),
        file: file!(),
        line: line!(),
    })?;

    let caps = re.captures(sql.trim())
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: sql.to_string(),
            reason: "Could not parse DROP TVIEW. Syntax: DROP TVIEW [IF EXISTS] name".to_string(),
        })?;

    let if_exists = caps.get(1).is_some();
    let schema_name = caps.get(2).map(|m| m.as_str().to_string());
    let tview_name = caps.get(3).unwrap().as_str().to_string();

    Ok(DropTViewStmt {
        tview_name,
        schema_name,
        if_exists,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let sql = "CREATE TVIEW tv_post AS SELECT * FROM tb_post";
        let parsed = parse_create_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert!(parsed.schema_name.is_none());
        assert!(parsed.select_sql.contains("SELECT"));
    }

    #[test]
    fn test_parse_with_schema() {
        let sql = "CREATE TVIEW public.tv_post AS SELECT pk_post FROM tb_post";
        let parsed = parse_create_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert_eq!(parsed.schema_name, Some("public".to_string()));
    }

    #[test]
    fn test_parse_multiline() {
        let sql = r#"
            CREATE TVIEW tv_post AS
            SELECT
                pk_post,
                id,
                data
            FROM tb_post
        "#;
        let parsed = parse_create_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert!(parsed.select_sql.contains("pk_post"));
    }

    #[test]
    fn test_parse_invalid_name() {
        let sql = "CREATE TVIEW bad_name AS SELECT * FROM tb";
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
    fn test_parse_drop_simple() {
        let sql = "DROP TVIEW tv_post";
        let parsed = parse_drop_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert!(!parsed.if_exists);
    }

    #[test]
    fn test_parse_drop_if_exists() {
        let sql = "DROP TVIEW IF EXISTS tv_post";
        let parsed = parse_drop_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert!(parsed.if_exists);
    }
}

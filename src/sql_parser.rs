//! SQL Parser for extracting cascade paths from view definitions.
//!
//! Uses sqlparser to analyze JOIN relationships and build cascade paths
//! from leaf tables back to root tables for incremental refresh.

use crate::cascade_path::{CascadeHop, CascadePath};
use sqlparser::ast::{Join, JoinOperator, Query, Select, SetExpr, Statement, TableFactor, TableWithJoins};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::{Parser, ParserError};

#[allow(dead_code)] // Reason: Will be used in future cascade implementation phases

/// Result type for SQL parsing operations
#[allow(dead_code)] // Reason: Will be used in future cascade implementation phases
pub type SqlParseResult<T> = Result<T, SqlParseError>;

/// Errors that can occur during SQL parsing
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Reason: Will be used in future cascade implementation phases
pub enum SqlParseError {
    /// SQL syntax error
    ParseError(String),
    /// Unsupported SQL construct (subqueries, LATERAL, etc.)
    UnsupportedConstruct(String),
    /// Multiple root tables found
    MultipleRoots,
    /// No root table found
    NoRootFound,
    /// Table alias resolution failed
    AliasResolutionError(String),
}

impl From<ParserError> for SqlParseError {
    fn from(err: ParserError) -> Self {
        SqlParseError::ParseError(err.to_string())
    }
}

/// Extract cascade paths from a SELECT statement
///
/// Given a SELECT SQL string, root table name, and root primary key column,
/// returns all cascade paths from leaf tables back to the root table.
///
/// # Arguments
/// * `select_sql` - The SELECT statement to parse
/// * `root_table` - Name of the root table (e.g., "tb_order")
/// * `root_pk_col` - Primary key column of the root table (e.g., "pk_order")
///
/// # Returns
/// Vector of cascade paths, one for each leaf table that can be reached from root
#[allow(dead_code)] // Reason: Will be implemented in future cascade cycles
pub fn extract_cascade_paths(
    select_sql: &str,
    root_table: &str,
    root_pk_col: &str,
) -> SqlParseResult<Vec<CascadePath>> {
    let dialect = PostgreSqlDialect {};
    let mut parser = Parser::new(&dialect).try_with_sql(select_sql)?;

    let stmt = parser.parse_statement()?;
    let Statement::Query(query) = stmt else {
        return Err(SqlParseError::UnsupportedConstruct(
            "Only SELECT queries are supported".to_string(),
        ));
    };

    extract_from_query(&query, root_table, root_pk_col)
}

fn extract_from_query(
    query: &Query,
    root_table: &str,
    root_pk_col: &str,
) -> SqlParseResult<Vec<CascadePath>> {
    let SetExpr::Select(select) = &*query.body else {
        return Err(SqlParseError::UnsupportedConstruct(
            "UNION and other set operations not yet supported".to_string(),
        ));
    };

    extract_from_select(select, root_table, root_pk_col)
}

fn extract_from_select(
    select: &Select,
    root_table: &str,
    root_pk_col: &str,
) -> SqlParseResult<Vec<CascadePath>> {
    // For cycle 2, we'll implement basic single-hop extraction
    // Start with the FROM clause
    if let Some(from) = &select.from.first() {
        return extract_from_table_with_joins(from, root_table, root_pk_col);
    }

    Ok(vec![])
}

fn extract_from_table_with_joins(
    table_with_joins: &TableWithJoins,
    root_table: &str,
    root_pk_col: &str,
) -> SqlParseResult<Vec<CascadePath>> {
    // For cycle 2: simple case - one root table with one JOIN
    // We'll extract the relationship and create a basic cascade path

    // Get the root table name (resolve aliases if present)
    let root_table_name = match &table_with_joins.relation {
        sqlparser::ast::TableFactor::Table { name, .. } => {
            name.0.last().map(|ident| ident.value.clone())
                .ok_or(SqlParseError::NoRootFound)?
        }
        _ => return Err(SqlParseError::UnsupportedConstruct("Only table factors supported".to_string())),
    };

    // Check if this is our root table
    if root_table_name != root_table {
        return Ok(vec![]); // Not our root table
    }

    let mut paths = Vec::new();

    // Process each JOIN
    for join in &table_with_joins.joins {
        if let Some(path) = extract_join_path(join, &root_table_name, root_pk_col)? {
            paths.push(path);
        }
    }

    Ok(paths)
}

fn extract_join_path(
    join: &Join,
    _root_table: &str,
    _root_pk_col: &str,
) -> SqlParseResult<Option<CascadePath>> {
    // For cycle 2: extract simple JOIN relationship
    // Only handle INNER JOIN and basic ON conditions for now

    match join.join_operator {
        JoinOperator::Inner(_) => {
            // Good, we can handle INNER JOIN
        }
        _ => return Ok(None), // Skip other join types for now
    }

    // Get the joined table name
    let join_table_name = match &join.relation {
        TableFactor::Table { name, .. } => {
            name.0.last().map(|ident| ident.value.clone())
                .ok_or(SqlParseError::AliasResolutionError("Invalid table name".to_string()))?
        }
        _ => return Ok(None), // Skip complex table factors
    };

    // For now, create a simple cascade path
    // This is a placeholder - we'll extract actual FK relationships in later cycles
    let hop = CascadeHop {
        source_oid: pgrx::pg_sys::Oid::INVALID, // Will be resolved later
        target_entity: join_table_name.clone(),
    };

    let path = CascadePath {
        hops: vec![hop],
    };

    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_cascade_paths_basic_parsing() {
        let sql = "SELECT 1 FROM tb_a JOIN tb_b ON tb_b.fk = tb_a.pk";
        let result = extract_cascade_paths(sql, "tb_a", "pk");
        // Should not error on basic parsing
        assert!(result.is_ok());
        // For now returns empty vec
        assert_eq!(result.unwrap(), vec![]);
    }

    #[test]
    fn test_extract_cascade_paths_unsupported_construct() {
        let sql = "SELECT 1 FROM (SELECT * FROM tb_a) sub";
        let result = extract_cascade_paths(sql, "tb_a", "pk");
        assert!(matches!(result, Err(SqlParseError::UnsupportedConstruct(_))));
    }

    #[test]
    fn test_extract_cascade_paths_single_join() {
        // Test for cycle 2: simple single JOIN
        let sql = "SELECT 1 FROM tb_order o JOIN tb_item i ON o.pk_order = i.fk_order";
        let result = extract_cascade_paths(sql, "tb_order", "pk_order");

        // Should succeed and return at least one path
        assert!(result.is_ok());
        let paths = result.unwrap();
        assert!(!paths.is_empty(), "Should find at least one cascade path");

        // Check the path structure
        let path = &paths[0];
        assert_eq!(path.hops.len(), 1);
        assert_eq!(path.hops[0].target_entity, "tb_item");
    }
}
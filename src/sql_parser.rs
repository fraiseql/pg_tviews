//! SQL Parser for extracting cascade paths from view definitions.
//!
//! Uses sqlparser to analyze JOIN relationships and build cascade paths
//! from leaf tables back to root tables for incremental refresh.

use crate::cascade_path::{CascadeHop, CascadePath};
use sqlparser::ast::{BinaryOperator, Expr, Join, JoinOperator, Query, Select, SetExpr, Statement, TableFactor, TableWithJoins};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::{Parser, ParserError};
use std::collections::{HashMap, HashSet, VecDeque};

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

/// Represents a JOIN relationship between two tables
#[derive(Debug, Clone)]
#[allow(dead_code)] // Reason: Will be used in future ON condition parsing
struct JoinEdge {
    left_table: String,
    #[allow(dead_code)] // Reason: Will be used for FK relationship analysis
    left_column: String,
    right_table: String,
    #[allow(dead_code)] // Reason: Will be used for FK relationship analysis
    right_column: String,
}

/// Graph representation of table relationships
#[derive(Debug)]
struct TableGraph {
    /// Maps table name -> set of tables it joins to
    adjacency: HashMap<String, HashSet<String>>,
    /// Maps (table_a, table_b) -> JoinEdge
    edges: HashMap<(String, String), JoinEdge>,
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
    _root_pk_col: &str,
) -> SqlParseResult<Vec<CascadePath>> {
    // For cycle 4: handle edge cases including implicit joins
    if select.from.is_empty() {
        return Ok(vec![]);
    }

    let from = &select.from[0];
    let mut graph = build_table_graph(from)?;

    // Also check WHERE clause for implicit joins
    if let Some(where_clause) = &select.selection {
        extract_implicit_joins(where_clause, &mut graph)?;
    }

    // Find the root table in our graph
    if !graph.adjacency.contains_key(root_table) {
        return Ok(vec![]); // Root table not found in this query
    }

    // Find all leaf tables (tables that are not referenced as JOIN sources)
    let all_tables: HashSet<String> = graph.adjacency.keys().cloned().collect();
    let mut join_sources = HashSet::new();

    // The root table is always a source
    join_sources.insert(root_table.to_string());

    // Add all tables that appear as the left side of any join
    for edge in graph.edges.values() {
        join_sources.insert(edge.left_table.clone());
    }

    let leaf_tables: Vec<String> = all_tables
        .difference(&join_sources)
        .cloned()
        .collect();

    // For each leaf table, find path back to root
    let mut paths = Vec::new();
    for leaf in leaf_tables {
        if let Some(path) = find_path_to_root(&graph, &leaf, root_table) {
            paths.push(path);
        }
    }

    Ok(paths)
}

fn build_table_graph(table_with_joins: &TableWithJoins) -> SqlParseResult<TableGraph> {
    let mut adjacency: HashMap<String, HashSet<String>> = HashMap::new();
    let mut edges: HashMap<(String, String), JoinEdge> = HashMap::new();
    let mut table_aliases: HashMap<String, String> = HashMap::new(); // alias -> actual_table

    // Start with the root table
    let (root_table, root_alias) = extract_table_info(&table_with_joins.relation)?;
    if let Some(alias) = root_alias {
        table_aliases.insert(alias, root_table.clone());
    }

    // Process each JOIN
    for join in &table_with_joins.joins {
        // Handle different join types
        let is_supported_join = matches!(join.join_operator,
            JoinOperator::Inner(_) | JoinOperator::LeftOuter(_));

        if !is_supported_join {
            // For now, skip unsupported join types (RIGHT, FULL OUTER, etc.)
            continue;
        }

        let (right_table, right_alias) = extract_table_info(&join.relation)?;
        if let Some(alias) = right_alias.clone() {
            table_aliases.insert(alias, right_table.clone());
        }

        let join_edge = extract_join_condition(&join, &root_table, &right_table)?;

        // Add bidirectional adjacency
        adjacency.entry(join_edge.left_table.clone())
            .or_insert_with(HashSet::new)
            .insert(join_edge.right_table.clone());
        adjacency.entry(join_edge.right_table.clone())
            .or_insert_with(HashSet::new)
            .insert(join_edge.left_table.clone());

        // Store the edge
        let key = if join_edge.left_table < join_edge.right_table {
            (join_edge.left_table.clone(), join_edge.right_table.clone())
        } else {
            (join_edge.right_table.clone(), join_edge.left_table.clone())
        };
        edges.insert(key, join_edge);
    }

    Ok(TableGraph { adjacency, edges })
}

fn extract_table_info(table_factor: &TableFactor) -> SqlParseResult<(String, Option<String>)> {
    match table_factor {
        TableFactor::Table { name, alias, .. } => {
            let table_name = name.0.last()
                .map(|ident| ident.value.clone())
                .ok_or(SqlParseError::AliasResolutionError("Invalid table name".to_string()))?;

            let alias_name = alias.as_ref().map(|a| a.name.value.clone());

            Ok((table_name, alias_name))
        }
        _ => Err(SqlParseError::UnsupportedConstruct("Complex table factors not supported".to_string())),
    }
}

#[allow(dead_code)] // Reason: May be used in future refactoring
fn extract_table_name(table_factor: &TableFactor) -> SqlParseResult<String> {
    extract_table_info(table_factor).map(|(name, _alias)| name)
}

fn extract_join_condition(_join: &Join, left_table: &str, right_table: &str) -> SqlParseResult<JoinEdge> {
    // For now, create a simple edge - we'll parse actual ON conditions later
    // This is a placeholder for the FK relationship extraction
    Ok(JoinEdge {
        left_table: left_table.to_string(),
        left_column: "id".to_string(), // Placeholder
        right_table: right_table.to_string(),
        right_column: "fk_id".to_string(), // Placeholder
    })
}

fn extract_implicit_joins(where_expr: &Expr, graph: &mut TableGraph) -> SqlParseResult<()> {
    // For cycle 4: extract implicit joins from WHERE clause equality conditions
    // This is a simplified implementation - in practice, we'd need more sophisticated
    // expression parsing to handle complex WHERE clauses

    match where_expr {
        Expr::BinaryOp { left, op, right } if matches!(op, BinaryOperator::Eq) => {
            // Check if this is a table.column = table.column condition
            if let (Some(left_col), Some(right_col)) = (extract_column_ref(left), extract_column_ref(right)) {
                if left_col.table != right_col.table {
                    // Found an implicit join!
                    let edge = JoinEdge {
                        left_table: left_col.table.clone(),
                        left_column: left_col.column,
                        right_table: right_col.table.clone(),
                        right_column: right_col.column,
                    };

                    // Add to graph
                    graph.adjacency.entry(edge.left_table.clone())
                        .or_insert_with(HashSet::new)
                        .insert(edge.right_table.clone());
                    graph.adjacency.entry(edge.right_table.clone())
                        .or_insert_with(HashSet::new)
                        .insert(edge.left_table.clone());

                    let key = if edge.left_table < edge.right_table {
                        (edge.left_table.clone(), edge.right_table.clone())
                    } else {
                        (edge.right_table.clone(), edge.left_table.clone())
                    };
                    graph.edges.insert(key, edge);
                }
            }
        }
        // For more complex expressions, we'd recurse into AND/OR conditions
        // But for cycle 4, we'll keep it simple
        _ => {}
    }

    Ok(())
}

#[derive(Debug)]
struct ColumnRef {
    table: String,
    column: String,
}

fn extract_column_ref(expr: &Expr) -> Option<ColumnRef> {
    match expr {
        Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
            Some(ColumnRef {
                table: parts[0].value.clone(),
                column: parts[1].value.clone(),
            })
        }
        _ => None,
    }
}

fn find_path_to_root(graph: &TableGraph, start: &str, root: &str) -> Option<CascadePath> {
    // BFS from start to root
    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();
    let mut parent_map: HashMap<String, String> = HashMap::new();

    queue.push_back(start.to_string());
    visited.insert(start.to_string());

    while let Some(current) = queue.pop_front() {
        if current == root {
            // Found path! Reconstruct it
            return Some(reconstruct_path(&parent_map, start, root));
        }

        if let Some(neighbors) = graph.adjacency.get(&current) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    visited.insert(neighbor.clone());
                    parent_map.insert(neighbor.clone(), current.clone());
                    queue.push_back(neighbor.clone());
                }
            }
        }
    }

    None // No path found
}

fn reconstruct_path(parent_map: &HashMap<String, String>, start: &str, root: &str) -> CascadePath {
    let mut path = vec![];
    let mut current = start;

    while current != root {
        if let Some(parent) = parent_map.get(current) {
            // Create hop from parent to current
            let hop = CascadeHop {
                source_oid: pgrx::pg_sys::Oid::INVALID, // Will be resolved later
                target_entity: current.to_string(),
            };
            path.push(hop);
            current = parent;
        } else {
            break;
        }
    }

    // Reverse to get root -> leaf order
    path.reverse();

    CascadePath { hops: path }
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

    #[test]
    fn test_extract_cascade_paths_multi_hop() {
        // Test for cycle 3: multi-hop JOIN chain
        let sql = "SELECT 1 FROM tb_order o \
                   JOIN tb_group g ON o.pk_order = g.fk_order \
                   JOIN tb_item i ON g.pk_group = i.fk_group";
        let result = extract_cascade_paths(sql, "tb_order", "pk_order");

        // Should succeed and return paths
        assert!(result.is_ok());
        let paths = result.unwrap();

        // Should find tb_item as a leaf table
        let item_path = paths.iter().find(|p| {
            p.hops.last().map(|h| h.target_entity == "tb_item").unwrap_or(false)
        });
        assert!(item_path.is_some(), "Should find path to tb_item");

        let path = item_path.unwrap();
        // Should have 2 hops: order -> group -> item
        assert_eq!(path.hops.len(), 2);
        assert_eq!(path.hops[0].target_entity, "tb_group");
        assert_eq!(path.hops[1].target_entity, "tb_item");
    }

    #[test]
    fn test_extract_cascade_paths_with_aliases() {
        // Test for cycle 4: table aliases
        let sql = "SELECT 1 FROM tb_order o JOIN tb_item i ON o.pk_order = i.fk_order";
        let result = extract_cascade_paths(sql, "tb_order", "pk_order");

        // Should work with aliases
        assert!(result.is_ok());
        let paths = result.unwrap();
        assert!(!paths.is_empty());
    }

    #[test]
    fn test_extract_cascade_paths_left_join() {
        // Test for cycle 4: LEFT JOIN support
        let sql = "SELECT 1 FROM tb_order o LEFT JOIN tb_item i ON o.pk_order = i.fk_order";
        let result = extract_cascade_paths(sql, "tb_order", "pk_order");

        // Should handle LEFT JOIN
        assert!(result.is_ok());
        let paths = result.unwrap();
        assert!(!paths.is_empty());
    }

    #[test]
    fn test_extract_cascade_paths_implicit_join() {
        // Test for cycle 4: implicit joins in WHERE clause
        let sql = "SELECT 1 FROM tb_order o, tb_item i WHERE o.pk_order = i.fk_order";
        let result = extract_cascade_paths(sql, "tb_order", "pk_order");

        // Should handle implicit joins (comma syntax with WHERE condition)
        // Note: This test may fail initially since we don't parse comma joins yet
        // But the framework should be in place
        assert!(result.is_ok());
    }
}
//! SQL Parser for extracting join paths from view definitions.
//!
//! Parses the FROM/JOIN clause of a SELECT statement to build hop chains
//! from each leaf table back to the root table. These paths are later
//! resolved to OIDs and stored as `CascadePath` entries in `pg_tview_meta`.

use sqlparser::ast::{
    BinaryOperator, Expr, JoinConstraint, JoinOperator, SetExpr, Statement, TableFactor,
    TableWithJoins,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use std::collections::{HashMap, HashSet, VecDeque};

/// A join path from a leaf table back to the root, with column names only (no OIDs).
/// Produced by the SQL parser; resolved to `CascadePath` at registration time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinPath {
    /// Leaf table name (where the trigger fires)
    pub source_table: String,
    /// Column on the leaf table to read from the changed row
    pub initial_col: String,
    /// Intermediate hops to reach the root table's PK
    pub steps: Vec<JoinStep>,
    /// Column on the root table that the last hop (or initial_col) connects to.
    /// Used by the resolver to detect when a reverse-lookup hop through the
    /// root table is needed (i.e. when this is NOT the entity PK column).
    pub root_join_col: String,
}

/// One intermediate SPI query step in a join path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinStep {
    /// Table to query
    pub table_name: String,
    /// Column for WHERE clause (matched against incoming IDs)
    pub lookup_col: String,
    /// Column to SELECT (carried forward to next step or final PK)
    pub carry_col: String,
}

/// A directed edge in the join graph: left.left_col = right.right_col
#[derive(Debug, Clone)]
struct JoinEdge {
    left_table: String,
    left_col: String,
    right_table: String,
    right_col: String,
}

/// Adjacency graph of table join relationships
#[derive(Debug)]
struct JoinGraph {
    /// All known table names
    tables: HashSet<String>,
    /// Edges keyed by unordered pair for fast lookup
    edges: Vec<JoinEdge>,
    /// Alias → real table name
    aliases: HashMap<String, String>,
}

impl JoinGraph {
    fn new() -> Self {
        Self {
            tables: HashSet::new(),
            edges: Vec::new(),
            aliases: HashMap::new(),
        }
    }

    fn add_table(&mut self, name: &str, alias: Option<&str>) {
        self.tables.insert(name.to_string());
        if let Some(a) = alias {
            self.aliases.insert(a.to_string(), name.to_string());
        }
    }

    /// Resolve an identifier (alias or table name) to the real table name
    fn resolve(&self, name: &str) -> String {
        self.aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }

    fn add_edge(&mut self, edge: JoinEdge) {
        self.edges.push(edge);
    }

    /// Find all edges connecting two tables (in either direction)
    fn edges_between(&self, a: &str, b: &str) -> Vec<&JoinEdge> {
        self.edges
            .iter()
            .filter(|e| {
                (e.left_table == a && e.right_table == b)
                    || (e.left_table == b && e.right_table == a)
            })
            .collect()
    }

    /// Get neighbors of a table
    fn neighbors(&self, table: &str) -> HashSet<String> {
        let mut result = HashSet::new();
        for edge in &self.edges {
            if edge.left_table == table {
                result.insert(edge.right_table.clone());
            } else if edge.right_table == table {
                result.insert(edge.left_table.clone());
            }
        }
        result
    }
}

/// Extract join paths from a SELECT statement.
///
/// Returns one `JoinPath` per non-root table in the FROM/JOIN clause,
/// describing how to traverse from that table back to `root_table`.
///
/// Returns an empty vec if the root table is not found in the query.
pub fn extract_join_paths(select_sql: &str, root_table: &str) -> Result<Vec<JoinPath>, String> {
    let dialect = PostgreSqlDialect {};
    let stmts = Parser::new(&dialect)
        .try_with_sql(select_sql)
        .map_err(|e| format!("SQL init error: {e}"))?
        .parse_statements()
        .map_err(|e| format!("SQL parse error: {e}"))?;

    // Handle bare SELECT (no statement wrapper) — try wrapping in SELECT if needed
    let stmt = stmts
        .into_iter()
        .next()
        .ok_or_else(|| "Empty SQL statement".to_string())?;

    let query = match stmt {
        Statement::Query(q) => q,
        _ => return Err("Only SELECT queries are supported".to_string()),
    };

    let select = match *query.body {
        SetExpr::Select(s) => s,
        _ => return Err("UNION/set operations not yet supported for cascade paths".to_string()),
    };

    if select.from.is_empty() {
        return Ok(vec![]);
    }

    // Build join graph from FROM clause
    let mut graph = JoinGraph::new();

    for table_with_joins in &select.from {
        build_graph_from_table_with_joins(table_with_joins, &mut graph)?;
    }

    // Also extract implicit joins from WHERE clause
    if let Some(ref where_expr) = select.selection {
        extract_implicit_joins(where_expr, &mut graph);
    }

    if !graph.tables.contains(root_table) {
        return Ok(vec![]);
    }

    // Find all non-root tables and compute paths to root
    let non_root_tables: Vec<String> = graph
        .tables
        .iter()
        .filter(|t| *t != root_table)
        .cloned()
        .collect();

    let mut paths = Vec::new();
    for leaf in &non_root_tables {
        if let Some(path) = build_path(&graph, leaf, root_table) {
            paths.push(path);
        }
    }

    Ok(paths)
}

/// Build the join graph from a single FROM clause entry
fn build_graph_from_table_with_joins(
    twj: &TableWithJoins,
    graph: &mut JoinGraph,
) -> Result<(), String> {
    // Extract root table
    let (root_name, root_alias) = extract_table_info(&twj.relation)?;
    graph.add_table(&root_name, root_alias.as_deref());

    // Process each JOIN
    for join in &twj.joins {
        let (right_name, right_alias) = extract_table_info(&join.relation)?;
        graph.add_table(&right_name, right_alias.as_deref());

        // Extract ON condition columns
        let constraint = match &join.join_operator {
            JoinOperator::Inner(c)
            | JoinOperator::LeftOuter(c)
            | JoinOperator::RightOuter(c)
            | JoinOperator::FullOuter(c) => c,
            _ => continue, // CROSS JOIN etc. — no ON condition
        };

        match constraint {
            JoinConstraint::On(expr) => {
                extract_equalities(expr, graph);
            }
            JoinConstraint::Using(cols) => {
                // USING(col) means left.col = right.col
                // We need to figure out which tables. Use the most recently added tables.
                for col in cols {
                    let col_name = col.value.clone();
                    // For USING, both sides have the same column name.
                    // The "left" is whatever was before this JOIN (could be root or previous join).
                    // For simplicity, we don't know the exact left table here,
                    // so we record both with the right table name and hope BFS resolves it.
                    // In practice, USING joins are rare in TVIEW definitions.
                    graph.add_edge(JoinEdge {
                        left_table: root_name.clone(),
                        left_col: col_name.clone(),
                        right_table: right_name.clone(),
                        right_col: col_name,
                    });
                }
            }
            JoinConstraint::Natural | JoinConstraint::None => {}
        }
    }

    Ok(())
}

/// Extract table name and alias from a `TableFactor`
fn extract_table_info(factor: &TableFactor) -> Result<(String, Option<String>), String> {
    match factor {
        TableFactor::Table { name, alias, .. } => {
            let table_name = name
                .0
                .last()
                .map(|i| i.value.clone())
                .ok_or("Invalid table name")?;
            let alias_name = alias.as_ref().map(|a| a.name.value.clone());
            Ok((table_name, alias_name))
        }
        _ => {
            Err("Subqueries and derived tables not supported in cascade path analysis".to_string())
        }
    }
}

/// Recursively extract equality conditions from an expression and add as edges.
/// Handles AND chains: `a.x = b.y AND c.z = d.w`
fn extract_equalities(expr: &Expr, graph: &mut JoinGraph) {
    match expr {
        Expr::BinaryOp { left, op, right } => match op {
            BinaryOperator::Eq => {
                if let (Some(left_ref), Some(right_ref)) =
                    (extract_col_ref(left, graph), extract_col_ref(right, graph))
                    && left_ref.0 != right_ref.0
                {
                    graph.add_edge(JoinEdge {
                        left_table: left_ref.0,
                        left_col: left_ref.1,
                        right_table: right_ref.0,
                        right_col: right_ref.1,
                    });
                }
            }
            BinaryOperator::And => {
                extract_equalities(left, graph);
                extract_equalities(right, graph);
            }
            _ => {}
        },
        Expr::Nested(inner) => extract_equalities(inner, graph),
        _ => {}
    }
}

/// Extract a `table.column` reference from an expression, resolving aliases.
/// Returns `(resolved_table_name, column_name)`.
fn extract_col_ref(expr: &Expr, graph: &JoinGraph) -> Option<(String, String)> {
    match expr {
        Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
            let table = graph.resolve(&parts[0].value);
            let column = parts[1].value.clone();
            Some((table, column))
        }
        _ => None,
    }
}

/// Extract implicit joins from WHERE clause equality conditions
fn extract_implicit_joins(expr: &Expr, graph: &mut JoinGraph) {
    extract_equalities(expr, graph);
}

/// BFS from `leaf` to `root` through the join graph, then reconstruct
/// the `JoinPath` with proper column assignments.
fn build_path(graph: &JoinGraph, leaf: &str, root: &str) -> Option<JoinPath> {
    // BFS to find shortest path
    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();
    let mut parent_map: HashMap<String, String> = HashMap::new();

    queue.push_back(leaf.to_string());
    visited.insert(leaf.to_string());

    while let Some(current) = queue.pop_front() {
        if current == root {
            break;
        }
        for neighbor in graph.neighbors(&current) {
            if !visited.contains(&neighbor) {
                visited.insert(neighbor.clone());
                parent_map.insert(neighbor.clone(), current.clone());
                queue.push_back(neighbor);
            }
        }
    }

    if !visited.contains(root) {
        return None; // No path found
    }

    // Reconstruct path: root → ... → leaf (we reverse at the end)
    let mut chain = vec![root.to_string()];
    let mut current = root.to_string();
    // Walk parent_map from root back to leaf
    // Actually parent_map stores child→parent (in BFS from leaf),
    // so parent_map[root] = previous node toward leaf.
    // We need to walk from root via parent_map, but parent_map points toward leaf.
    // Let me re-think: BFS starts at leaf. parent_map[X] = the node that discovered X.
    // So to reconstruct: start at root, follow parent_map to leaf.
    loop {
        if current == leaf {
            break;
        }
        let prev = parent_map.get(&current)?;
        chain.push(prev.clone());
        current = prev.clone();
    }
    // chain is now [root, ..., leaf]

    if chain.len() < 2 {
        return None;
    }

    // Build JoinPath by walking from leaf toward root
    // chain = [root, intermediate..., leaf]
    // We walk pairs from the leaf end: (leaf, next), (next, next2), ..., (penultimate, root)
    chain.reverse(); // now [leaf, ..., root]

    // First pair: leaf → next_table
    let first_edge = graph.edges_between(&chain[0], &chain[1]);
    let first_edge = first_edge.first()?;

    // initial_col: the column on the leaf table side
    let initial_col = if first_edge.left_table == chain[0] {
        first_edge.left_col.clone()
    } else {
        first_edge.right_col.clone()
    };

    // For each intermediate table, build a JoinStep
    let mut steps = Vec::new();
    for i in 1..chain.len() - 1 {
        let table = &chain[i];

        // Edge from previous table to this table (how we arrive)
        let incoming_edge = graph.edges_between(&chain[i - 1], table);
        let incoming = incoming_edge.first()?;
        // lookup_col: column on THIS table that matches incoming IDs
        let lookup_col = if incoming.left_table == *table {
            incoming.left_col.clone()
        } else {
            incoming.right_col.clone()
        };

        // Edge from this table to next table (how we leave)
        let outgoing_edge = graph.edges_between(table, &chain[i + 1]);
        let outgoing = outgoing_edge.first()?;
        // carry_col: column on THIS table to extract for next hop
        let carry_col = if outgoing.left_table == *table {
            outgoing.left_col.clone()
        } else {
            outgoing.right_col.clone()
        };

        steps.push(JoinStep {
            table_name: table.clone(),
            lookup_col,
            carry_col,
        });
    }

    // Extract the root table's column from the edge connecting to it.
    // This tells the resolver whether the path already terminates at the
    // entity PK or whether a reverse-lookup hop is needed.
    let last_idx = chain.len() - 1;
    let root_edge = graph.edges_between(&chain[last_idx - 1], &chain[last_idx]);
    let root_edge = root_edge.first()?;
    let root_join_col = if root_edge.left_table == chain[last_idx] {
        root_edge.left_col.clone()
    } else {
        root_edge.right_col.clone()
    };

    Some(JoinPath {
        source_table: chain[0].clone(),
        initial_col,
        steps,
        root_join_col,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_join() {
        let sql = "SELECT o.pk_order, g.name \
                   FROM tb_order o \
                   JOIN tb_group g ON g.fk_order = o.pk_order";

        let paths = extract_join_paths(sql, "tb_order").unwrap();
        assert_eq!(paths.len(), 1);

        let path = &paths[0];
        assert_eq!(path.source_table, "tb_group");
        assert_eq!(path.initial_col, "fk_order");
        assert!(
            path.steps.is_empty(),
            "single hop should have no intermediate steps"
        );
        assert_eq!(
            path.root_join_col, "pk_order",
            "root_join_col should be the root's PK (child→parent FK)"
        );
    }

    #[test]
    fn test_two_hop_chain() {
        let sql = "SELECT o.pk_order, g.name, i.value \
                   FROM tb_order o \
                   JOIN tb_group g ON g.fk_order = o.pk_order \
                   JOIN tb_item i ON i.fk_group = g.pk_group";

        let paths = extract_join_paths(sql, "tb_order").unwrap();

        // Should have paths for tb_group (direct) and tb_item (2-hop)
        assert_eq!(paths.len(), 2);

        let group_path = paths.iter().find(|p| p.source_table == "tb_group").unwrap();
        assert_eq!(group_path.initial_col, "fk_order");
        assert!(group_path.steps.is_empty());
        assert_eq!(group_path.root_join_col, "pk_order");

        let item_path = paths.iter().find(|p| p.source_table == "tb_item").unwrap();
        assert_eq!(item_path.initial_col, "fk_group");
        assert_eq!(item_path.steps.len(), 1);

        let hop = &item_path.steps[0];
        assert_eq!(hop.table_name, "tb_group");
        assert_eq!(hop.lookup_col, "pk_group");
        assert_eq!(hop.carry_col, "fk_order");
    }

    #[test]
    fn test_three_hop_chain() {
        let sql = "SELECT o.pk_order, g.name, i.value, d.detail \
                   FROM tb_order o \
                   JOIN tb_group g ON g.fk_order = o.pk_order \
                   JOIN tb_item i ON i.fk_group = g.pk_group \
                   JOIN tb_detail d ON d.fk_item = i.pk_item";

        let paths = extract_join_paths(sql, "tb_order").unwrap();

        let detail_path = paths
            .iter()
            .find(|p| p.source_table == "tb_detail")
            .unwrap();
        assert_eq!(detail_path.initial_col, "fk_item");
        assert_eq!(detail_path.steps.len(), 2);

        assert_eq!(detail_path.steps[0].table_name, "tb_item");
        assert_eq!(detail_path.steps[0].lookup_col, "pk_item");
        assert_eq!(detail_path.steps[0].carry_col, "fk_group");

        assert_eq!(detail_path.steps[1].table_name, "tb_group");
        assert_eq!(detail_path.steps[1].lookup_col, "pk_group");
        assert_eq!(detail_path.steps[1].carry_col, "fk_order");
    }

    #[test]
    fn test_aliases_resolved() {
        let sql = "SELECT 1 FROM tb_order o JOIN tb_group g ON g.fk_order = o.pk_order";
        let paths = extract_join_paths(sql, "tb_order").unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].source_table, "tb_group");
        assert_eq!(paths[0].initial_col, "fk_order");
    }

    #[test]
    fn test_left_join() {
        let sql = "SELECT 1 FROM tb_order o LEFT JOIN tb_group g ON g.fk_order = o.pk_order";
        let paths = extract_join_paths(sql, "tb_order").unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].source_table, "tb_group");
    }

    #[test]
    fn test_implicit_join() {
        let sql = "SELECT 1 FROM tb_order o, tb_group g WHERE g.fk_order = o.pk_order";
        let paths = extract_join_paths(sql, "tb_order").unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].source_table, "tb_group");
        assert_eq!(paths[0].initial_col, "fk_order");
    }

    #[test]
    fn test_subquery_returns_error() {
        let sql = "SELECT 1 FROM (SELECT * FROM tb_a) sub";
        let result = extract_join_paths(sql, "tb_a");
        assert!(result.is_err());
    }

    #[test]
    fn test_root_not_found_returns_empty() {
        let sql = "SELECT 1 FROM tb_order o JOIN tb_group g ON g.fk_order = o.pk_order";
        let paths = extract_join_paths(sql, "tb_nonexistent").unwrap();
        assert!(paths.is_empty());
    }

    /// Issue #007: When the root table holds the FK (lookup/dimension join),
    /// root_join_col must reflect the root's FK column, not its PK.
    /// The resolver uses this to detect the need for a reverse-lookup hop.
    #[test]
    fn test_lookup_table_join_root_holds_fk() {
        let sql = "SELECT o.pk_order, o.fk_currency, c.iso_code \
                   FROM tb_order o \
                   LEFT JOIN tb_currency c ON o.fk_currency = c.pk_currency";

        let paths = extract_join_paths(sql, "tb_order").unwrap();
        assert_eq!(paths.len(), 1);

        let path = &paths[0];
        assert_eq!(path.source_table, "tb_currency");
        assert_eq!(path.initial_col, "pk_currency");
        assert!(
            path.steps.is_empty(),
            "direct lookup join has no intermediate steps"
        );
        assert_eq!(
            path.root_join_col, "fk_currency",
            "root_join_col must be the FK on the root table, not its PK"
        );
    }
}

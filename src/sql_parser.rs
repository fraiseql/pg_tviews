//! SQL Parser for extracting join paths from view definitions.
//!
//! Parses the FROM/JOIN clause of a SELECT statement to build hop chains
//! from each leaf table back to the root table. These paths are later
//! resolved to OIDs and stored as `CascadePath` entries in `pg_tview_meta`.

use sqlparser::ast::{
    BinaryOperator, Distinct, Expr, JoinConstraint, JoinOperator, Select, SelectItem, SetExpr,
    SetOperator, Statement, TableFactor, TableWithJoins,
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
    /// Column on the root table that the last hop (or `initial_col`) connects to.
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

/// A directed edge in the join graph: `left.left_col` = `right.right_col`
#[derive(Debug, Clone)]
struct JoinEdge {
    left_table: String,
    left_col: String,
    right_table: String,
    right_col: String,
}

/// A CTE resolved to the single base table it reads, with a map from each of its
/// output columns to the base-table column that produces it. Only CTEs whose body
/// is a single SELECT over one base table (no joins, no set operation, no nesting)
/// are resolved; anything else is left unresolved so a reference to it produces no
/// cascade path rather than a wrong one.
#[derive(Debug, Clone)]
struct ResolvedCte {
    base_table: String,
    /// CTE output column name → base-table column name (passthrough columns only;
    /// aggregates / computed projections are absent).
    col_map: HashMap<String, String>,
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
    /// Column-reference qualifier (CTE name or its FROM alias) → resolved CTE.
    /// Used to remap `cte.out_col` references to the underlying base column.
    cte_refs: HashMap<String, ResolvedCte>,
}

impl JoinGraph {
    fn new() -> Self {
        Self {
            tables: HashSet::new(),
            edges: Vec::new(),
            aliases: HashMap::new(),
            cte_refs: HashMap::new(),
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

    let entity = root_table.strip_prefix("tb_").unwrap_or(root_table);
    let pk_col = format!("pk_{entity}");

    // Resolve CTEs to their base tables so a reference through a WITH alias reaches
    // the underlying base table (issue #51). WITH RECURSIVE is rejected upstream at
    // create; here it simply yields no resolutions.
    let ctes = match &query.with {
        Some(with) if !with.recursive => resolve_ctes(with),
        _ => HashMap::new(),
    };

    // A UNION takes its output column names from the leftmost branch; find the
    // projection position that produces pk_<entity> so a root-less branch (one that
    // does not read tb_<entity> directly) can be connected back to the entity PK.
    let pk_position = leftmost_select(&query.body).and_then(|s| pk_output_position(s, &pk_col));

    let mut paths = Vec::new();
    collect_paths_from_setexpr(
        &query.body,
        root_table,
        &pk_col,
        pk_position,
        false,
        &ctes,
        &mut paths,
    )?;
    Ok(dedup_join_paths(paths))
}

/// Resolve the CTEs declared in a `WITH` clause to their base tables. Only CTEs
/// whose body is a single SELECT over exactly one base table (no joins, no set
/// operation, and not reading another CTE) are resolved; the rest are omitted so
/// references to them stay unresolvable (no wrong cascade path is produced).
fn resolve_ctes(with: &sqlparser::ast::With) -> HashMap<String, ResolvedCte> {
    let mut resolved: HashMap<String, ResolvedCte> = HashMap::new();
    for cte in &with.cte_tables {
        let name = cte.alias.name.value.clone();
        if let Some(rc) = resolve_single_cte(cte, &resolved) {
            resolved.insert(name, rc);
        }
    }
    resolved
}

/// Resolve one CTE to `(base_table, output_col -> base_col)` when its body is a
/// single SELECT over one base table. Returns `None` for multi-table, set-op,
/// or CTE-on-CTE bodies (out of v1 scope).
fn resolve_single_cte(
    cte: &sqlparser::ast::Cte,
    already_resolved: &HashMap<String, ResolvedCte>,
) -> Option<ResolvedCte> {
    let SetExpr::Select(select) = &*cte.query.body else {
        return None; // set-operation body — out of scope
    };
    if select.from.len() != 1 {
        return None;
    }
    let twj = &select.from[0];
    if !twj.joins.is_empty() {
        return None; // multi-base body — out of scope
    }
    let (from_name, _) = extract_table_info(&twj.relation).ok()?;
    if already_resolved.contains_key(&from_name) {
        return None; // reads another CTE — out of scope (no chaining in v1)
    }

    // Build the output-column → base-column map. WITH-declared column aliases
    // (`WITH c(a, b) AS ...`) rename outputs positionally; otherwise the output
    // name is the projection item's own name.
    let declared = &cte.alias.columns;
    let mut col_map = HashMap::new();
    for (i, item) in select.projection.iter().enumerate() {
        let (out_name, src_col) = match item {
            SelectItem::UnnamedExpr(e) => match expr_bare_column(e) {
                Some(col) => (col.clone(), col),
                None => continue, // aggregate / computed — not a passthrough
            },
            SelectItem::ExprWithAlias { expr, alias } => match expr_bare_column(expr) {
                Some(col) => (alias.value.clone(), col),
                None => continue,
            },
            _ => continue,
        };
        let out_name = declared.get(i).map_or(out_name, |ident| ident.value.clone());
        col_map.insert(out_name, src_col);
    }

    Some(ResolvedCte {
        base_table: from_name,
        col_map,
    })
}

/// True if the query begins with a `WITH RECURSIVE` clause. Recursive CTEs are not
/// supported for cascade tracking and are rejected at create time.
pub fn has_recursive_cte(select_sql: &str) -> bool {
    let dialect = PostgreSqlDialect {};
    let Ok(mut parser) = Parser::new(&dialect).try_with_sql(select_sql) else {
        return false;
    };
    let Ok(stmts) = parser.parse_statements() else {
        return false;
    };
    matches!(
        stmts.into_iter().next(),
        Some(Statement::Query(q)) if q.with.as_ref().is_some_and(|w| w.recursive)
    )
}

/// Recursively collect cascade `JoinPath`s from a query body, descending into
/// UNION branches. `in_set_op` becomes true once inside a set operation, enabling
/// the root-less branch connection for a UNION branch that does not read
/// `tb_<entity>` directly.
fn collect_paths_from_setexpr(
    body: &SetExpr,
    root_table: &str,
    pk_col: &str,
    pk_position: Option<usize>,
    in_set_op: bool,
    ctes: &HashMap<String, ResolvedCte>,
    out: &mut Vec<JoinPath>,
) -> Result<(), String> {
    match body {
        SetExpr::Select(select) => {
            out.extend(paths_for_select(
                select,
                root_table,
                pk_col,
                pk_position,
                in_set_op,
                ctes,
            )?);
            Ok(())
        }
        SetExpr::SetOperation {
            left, right, op, ..
        } => {
            // Only UNION [ALL] is supported for cascade tracking. INTERSECT / EXCEPT
            // have set-difference semantics the per-pk recompute does not model, so
            // reject them here rather than let them fall through as a UNION.
            if !matches!(op, SetOperator::Union) {
                return Err(format!(
                    "{op:?} set operations are not supported for cascade paths"
                ));
            }
            collect_paths_from_setexpr(left, root_table, pk_col, pk_position, true, ctes, out)?;
            collect_paths_from_setexpr(right, root_table, pk_col, pk_position, true, ctes, out)?;
            Ok(())
        }
        // Parenthesized branch / subquery body — recurse into it.
        SetExpr::Query(q) => {
            collect_paths_from_setexpr(&q.body, root_table, pk_col, pk_position, in_set_op, ctes, out)
        }
        _ => Err("Unsupported query body for cascade paths".to_string()),
    }
}

/// Build cascade paths for a single SELECT branch.
///
/// A branch that contains `root_table` uses the ordinary join-graph traversal. A
/// `root_table`-less UNION branch (`in_set_op`) is connected to the entity by
/// synthesizing an edge from the table that projects `pk_<entity>` (at
/// `pk_position`) to the entity PK. A plain SELECT without `root_table` yields no
/// paths (unchanged behaviour). Unresolvable branches are skipped, never producing
/// a wrong path.
fn paths_for_select(
    select: &Select,
    root_table: &str,
    pk_col: &str,
    pk_position: Option<usize>,
    in_set_op: bool,
    ctes: &HashMap<String, ResolvedCte>,
) -> Result<Vec<JoinPath>, String> {
    if select.from.is_empty() {
        return Ok(vec![]);
    }

    // Build join graph from FROM clause
    let mut graph = JoinGraph::new();
    for table_with_joins in &select.from {
        build_graph_from_table_with_joins(table_with_joins, &mut graph, ctes)?;
    }

    // Also extract implicit joins from WHERE clause
    if let Some(where_expr) = &select.selection {
        extract_implicit_joins(where_expr, &mut graph);
    }

    // Direct branch: the entity base table is present; traverse to it as usual.
    if graph.tables.contains(root_table) {
        return Ok(build_paths_to_root(&graph, root_table));
    }

    // A plain (non-UNION) SELECT without the root table has no cascade paths.
    if !in_set_op {
        return Ok(vec![]);
    }

    // Root-less UNION branch: connect the pk_<entity> provider to the entity PK.
    let Some(pk_pos) = pk_position else {
        return Ok(vec![]);
    };
    let Some((pk_table, pk_source_col)) = branch_pk_provider(select, pk_pos, &graph) else {
        return Ok(vec![]);
    };
    graph.add_table(root_table, None);
    graph.add_edge(JoinEdge {
        left_table: pk_table,
        left_col: pk_source_col,
        right_table: root_table.to_string(),
        right_col: pk_col.to_string(),
    });
    Ok(build_paths_to_root(&graph, root_table))
}

/// Build a `JoinPath` from every non-root table in the graph back to `root_table`.
fn build_paths_to_root(graph: &JoinGraph, root_table: &str) -> Vec<JoinPath> {
    graph
        .tables
        .iter()
        .filter(|t| *t != root_table)
        .filter_map(|leaf| build_path(graph, leaf, root_table))
        .collect()
}

/// Resolve the table and source column that provides the entity PK in a root-less
/// UNION branch (the projection item at `pk_position`).
///
/// Returns `None` for a wildcard, a non-column expression, or a bare column in a
/// multi-table branch (ambiguous ownership) — the branch is then skipped rather
/// than mapped to a guessed table.
fn branch_pk_provider(
    select: &Select,
    pk_position: usize,
    graph: &JoinGraph,
) -> Option<(String, String)> {
    let expr = match select.projection.get(pk_position)? {
        SelectItem::UnnamedExpr(e) => e,
        SelectItem::ExprWithAlias { expr, .. } => expr,
        _ => return None,
    };
    match expr {
        Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
            let table = graph.resolve(&parts[0].value);
            Some((table, parts[1].value.clone()))
        }
        Expr::Identifier(ident) => {
            // Bare column: unambiguous only when the branch reads a single table.
            if graph.tables.len() == 1 {
                graph
                    .tables
                    .iter()
                    .next()
                    .map(|t| (t.clone(), ident.value.clone()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Find the leftmost `Select` of a query body, descending through set operations
/// and parenthesized queries. UNION output column names come from this branch.
fn leftmost_select(body: &SetExpr) -> Option<&Select> {
    match body {
        SetExpr::Select(s) => Some(s),
        SetExpr::SetOperation { left, .. } => leftmost_select(left),
        SetExpr::Query(q) => leftmost_select(&q.body),
        _ => None,
    }
}

/// Return the projection index whose OUTPUT name is `pk_col`, if any.
fn pk_output_position(select: &Select, pk_col: &str) -> Option<usize> {
    select.projection.iter().position(|item| {
        let name = match item {
            SelectItem::ExprWithAlias { alias, .. } => Some(alias.value.clone()),
            SelectItem::UnnamedExpr(e) => expr_bare_column(e),
            _ => None,
        };
        name.as_deref() == Some(pk_col)
    })
}

/// Deduplicate `JoinPath`s by full structural equality, preserving order. Two
/// branches can legitimately produce different paths for the same source table,
/// so equality is over the whole path, not just `source_table`.
fn dedup_join_paths(paths: Vec<JoinPath>) -> Vec<JoinPath> {
    let mut unique: Vec<JoinPath> = Vec::with_capacity(paths.len());
    for p in paths {
        if !unique.contains(&p) {
            unique.push(p);
        }
    }
    unique
}

/// Build the join graph from a single FROM clause entry
fn build_graph_from_table_with_joins(
    twj: &TableWithJoins,
    graph: &mut JoinGraph,
    ctes: &HashMap<String, ResolvedCte>,
) -> Result<(), String> {
    // Extract root table
    let (root_name, root_alias) = extract_table_info(&twj.relation)?;
    add_table_or_cte(&root_name, root_alias.as_deref(), ctes, graph);

    // Process each JOIN
    for join in &twj.joins {
        let (right_name, right_alias) = extract_table_info(&join.relation)?;
        add_table_or_cte(&right_name, right_alias.as_deref(), ctes, graph);

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

/// Add a FROM/JOIN table to the graph, inlining a CTE reference to its base table.
///
/// For a CTE reference the base table becomes the graph node, and both the FROM
/// alias and the CTE name are registered in `cte_refs` so column references
/// (`alias.col` or `cte.col`) can be remapped to the base column in
/// `extract_col_ref`.
fn add_table_or_cte(
    name: &str,
    alias: Option<&str>,
    ctes: &HashMap<String, ResolvedCte>,
    graph: &mut JoinGraph,
) {
    if let Some(resolved) = ctes.get(name) {
        graph.tables.insert(resolved.base_table.clone());
        graph
            .aliases
            .insert(name.to_string(), resolved.base_table.clone());
        graph.cte_refs.insert(name.to_string(), resolved.clone());
        if let Some(a) = alias {
            graph
                .aliases
                .insert(a.to_string(), resolved.base_table.clone());
            graph.cte_refs.insert(a.to_string(), resolved.clone());
        }
    } else {
        graph.add_table(name, alias);
    }
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
///
/// When the qualifier is a CTE reference, the CTE output column is remapped to its
/// underlying base column; a reference to a non-passthrough CTE output (an
/// aggregate/computed column absent from the CTE's `col_map`) yields `None`, so no
/// (wrong) edge is created for it.
fn extract_col_ref(expr: &Expr, graph: &JoinGraph) -> Option<(String, String)> {
    match expr {
        Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
            let qualifier = &parts[0].value;
            let column = &parts[1].value;
            if let Some(cte) = graph.cte_refs.get(qualifier) {
                let base_col = cte.col_map.get(column)?;
                return Some((cte.base_table.clone(), base_col.clone()));
            }
            Some((graph.resolve(qualifier), column.clone()))
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

/// Extract the OUTPUT (projected) column name for each `DISTINCT ON` key.
///
/// Maps every `DISTINCT ON (expr)` to the SELECT-list item that projects it and
/// returns that item's output name — its `AS` alias, or the bare column name when
/// unaliased. This is the column the backing view and `tv_<entity>` actually
/// expose, so it is the name `refresh_by_dedup_key` must query and the tview's
/// primary key must use.
///
/// This is deliberately distinct from `schema::parser::extract_distinct_on_keys`,
/// which returns the raw SOURCE column read off the base tuple by the trigger.
/// When the `DISTINCT ON` key is aliased (`c.id_contract AS pk_contract`) the two
/// differ; both are stored so each consumer reads the correct one.
///
/// Returns `Ok(vec![])` when the query has no top-level `DISTINCT ON`.
///
/// # Errors
///
/// Returns `Err` if the SQL cannot be parsed, or if a `DISTINCT ON` key does not
/// map to a resolvable projected output column — the caller rejects the create,
/// because such a tview would build an invalid dedup key / primary key.
pub fn extract_distinct_on_output_keys(select_sql: &str) -> Result<Vec<String>, String> {
    let dialect = PostgreSqlDialect {};
    let stmts = Parser::new(&dialect)
        .try_with_sql(select_sql)
        .map_err(|e| format!("SQL init error: {e}"))?
        .parse_statements()
        .map_err(|e| format!("SQL parse error: {e}"))?;

    let Some(Statement::Query(query)) = stmts.into_iter().next() else {
        return Ok(vec![]);
    };

    // DISTINCT ON only applies to a plain SELECT body (not a set operation).
    let SetExpr::Select(select) = *query.body else {
        return Ok(vec![]);
    };

    let on_exprs = match &select.distinct {
        Some(Distinct::On(exprs)) => exprs,
        _ => return Ok(vec![]), // plain SELECT, or SELECT DISTINCT with no ON
    };

    let mut output_keys = Vec::with_capacity(on_exprs.len());
    for on_expr in on_exprs {
        let name = resolve_projection_output(on_expr, &select.projection).ok_or_else(|| {
            format!(
                "DISTINCT ON key `{on_expr}` is not projected under a resolvable output \
                 column; project it explicitly (e.g. `{on_expr} AS pk_<entity>`)"
            )
        })?;
        output_keys.push(name);
    }
    Ok(output_keys)
}

/// Resolve a `DISTINCT ON` expression to the output name of the SELECT-list item
/// that projects it. Matches structurally, or by bare column name when one side
/// is qualified (`t.col`) and the other is not (`col`).
fn resolve_projection_output(on_expr: &Expr, projection: &[SelectItem]) -> Option<String> {
    let on_col = expr_bare_column(on_expr);
    let matches =
        |expr: &Expr| expr == on_expr || (on_col.is_some() && expr_bare_column(expr) == on_col);
    for item in projection {
        match item {
            SelectItem::ExprWithAlias { expr, alias } if matches(expr) => {
                return Some(alias.value.clone());
            }
            // Unaliased projection: the output name IS the bare column name.
            SelectItem::UnnamedExpr(expr) if matches(expr) => {
                return expr_bare_column(expr);
            }
            _ => {} // wildcards / non-matching items carry no name to bind here
        }
    }
    None
}

/// Return the bare column name of a simple column reference (`col` or `t.col`).
/// `None` for non-column expressions (function calls, literals, casts, etc.).
fn expr_bare_column(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(ident) => Some(ident.value.clone()),
        Expr::CompoundIdentifier(parts) => parts.last().map(|i| i.value.clone()),
        _ => None,
    }
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
    /// `root_join_col` must reflect the root's FK column, not its PK.
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

    #[test]
    fn test_distinct_on_output_aliased() {
        // Aliased dedup key: output name is the alias, not the source column.
        let sql = "SELECT DISTINCT ON (c.id_contract) c.id_contract AS pk_contract, c.id \
                   FROM tb_contract c ORDER BY c.id_contract, c.version_no DESC";
        let keys = extract_distinct_on_output_keys(sql).unwrap();
        assert_eq!(keys, vec!["pk_contract"]);
    }

    #[test]
    fn test_distinct_on_output_unaliased() {
        // Bare projected key: output name == source column.
        let sql = "SELECT DISTINCT ON (c.id) c.id, c.pk_contract FROM tb_contract c";
        let keys = extract_distinct_on_output_keys(sql).unwrap();
        assert_eq!(keys, vec!["id"]);
    }

    #[test]
    fn test_distinct_on_output_qualifier_mismatch() {
        // DISTINCT ON unqualified, projection qualified: match by bare column name.
        let sql =
            "SELECT DISTINCT ON (id_contract) c.id_contract AS pk_contract FROM tb_contract c";
        let keys = extract_distinct_on_output_keys(sql).unwrap();
        assert_eq!(keys, vec!["pk_contract"]);
    }

    #[test]
    fn test_distinct_on_output_with_cte_preamble() {
        let sql = "WITH x AS (SELECT 1) \
                   SELECT DISTINCT ON (c.id_contract) c.id_contract AS pk_contract \
                   FROM tb_contract c";
        let keys = extract_distinct_on_output_keys(sql).unwrap();
        assert_eq!(keys, vec!["pk_contract"]);
    }

    #[test]
    fn test_distinct_on_output_none_when_no_distinct_on() {
        assert!(
            extract_distinct_on_output_keys("SELECT a, b FROM t")
                .unwrap()
                .is_empty()
        );
        // Plain DISTINCT (no ON) has no dedup key.
        assert!(
            extract_distinct_on_output_keys("SELECT DISTINCT a FROM t")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn test_distinct_on_output_unresolvable_expression_key() {
        // Expression key not projected under a resolvable name → Err (caller rejects).
        let sql = "SELECT DISTINCT ON (lower(c.code)) c.id FROM tb_contract c";
        assert!(extract_distinct_on_output_keys(sql).is_err());
    }

    #[test]
    fn test_union_cross_table_paths() {
        let sql = "SELECT pk_task, id, title FROM tb_task \
                   UNION ALL \
                   SELECT pk_task, id, title FROM tb_task_archive";
        let paths = extract_join_paths(sql, "tb_task").unwrap();
        // The direct branch (tb_task) needs no cascade path (direct entity trigger);
        // the archive branch must get one connecting its pk_task to the entity PK.
        let archive = paths
            .iter()
            .find(|p| p.source_table == "tb_task_archive")
            .expect("expected a cascade path for the UNION archive branch");
        assert_eq!(archive.initial_col, "pk_task");
        assert!(archive.steps.is_empty());
        assert_eq!(archive.root_join_col, "pk_task");
    }

    #[test]
    fn test_union_aliased_pk_position() {
        // Branch 2 provides the entity pk under a different source column, matched
        // by position (UNION output names come from the first branch).
        let sql = "SELECT pk_task, id FROM tb_task \
                   UNION ALL \
                   SELECT arch_id, id FROM tb_task_archive";
        let paths = extract_join_paths(sql, "tb_task").unwrap();
        let archive = paths
            .iter()
            .find(|p| p.source_table == "tb_task_archive")
            .expect("expected a cascade path for the aliased-position branch");
        assert_eq!(archive.initial_col, "arch_id");
        assert_eq!(archive.root_join_col, "pk_task");
    }

    #[test]
    fn test_intersect_rejected() {
        // INTERSECT / EXCEPT are not supported for cascade tracking.
        let sql = "SELECT pk_task FROM tb_task INTERSECT SELECT pk_task FROM tb_task_archive";
        assert!(extract_join_paths(sql, "tb_task").is_err());
    }

    #[test]
    fn test_cte_single_base_resolves() {
        let sql = "WITH cust_orders AS (\
                     SELECT fk_customer, count(*) AS n FROM tb_order GROUP BY fk_customer\
                   ) \
                   SELECT c.pk_customer, c.id FROM tb_customer c \
                   LEFT JOIN cust_orders co ON co.fk_customer = c.pk_customer";
        let paths = extract_join_paths(sql, "tb_customer").unwrap();
        let order = paths
            .iter()
            .find(|p| p.source_table == "tb_order")
            .expect("CTE-wrapped tb_order should resolve to a cascade path");
        assert_eq!(order.initial_col, "fk_customer");
        assert!(order.steps.is_empty());
        assert_eq!(order.root_join_col, "pk_customer");
    }

    #[test]
    fn test_cte_computed_join_column_unresolvable() {
        // Joining on a CTE aggregate output (not a passthrough base column) must not
        // synthesize a cascade path — leaving it unresolved is correct, a wrong path
        // is not.
        let sql = "WITH s AS (SELECT fk_customer, count(*) AS c FROM tb_order GROUP BY fk_customer) \
                   SELECT cust.pk_customer FROM tb_customer cust JOIN s ON s.c = cust.pk_customer";
        let paths = extract_join_paths(sql, "tb_customer").unwrap();
        assert!(
            paths.iter().all(|p| p.source_table != "tb_order"),
            "a join on a CTE aggregate must not produce a cascade path"
        );
    }

    #[test]
    fn test_recursive_cte_detected() {
        assert!(has_recursive_cte(
            "WITH RECURSIVE t AS (SELECT 1) SELECT pk_x FROM tb_x"
        ));
        assert!(!has_recursive_cte("SELECT pk_x FROM tb_x"));
        assert!(!has_recursive_cte(
            "WITH t AS (SELECT 1) SELECT pk_x FROM tb_x"
        ));
    }
}

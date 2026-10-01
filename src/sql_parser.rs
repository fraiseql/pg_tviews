//! SQL Parser for extracting join paths from view definitions.
//!
//! Parses the FROM/JOIN clause of a SELECT statement to build hop chains
//! from each leaf table back to the root table. These paths are later
//! resolved to OIDs and stored as `CascadePath` entries in `pg_tview_meta`.

use sqlparser::ast::{
    BinaryOperator, Distinct, Expr, JoinConstraint, JoinOperator, Select, SelectItem, SetExpr,
    Statement, TableFactor, TableWithJoins,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use std::collections::{HashMap, HashSet};

/// A directed edge in the join graph: `left.left_col` = `right.right_col`
#[derive(Debug, Clone)]
struct JoinEdge {
    left_table: String,
    left_col: String,
    right_table: String,
    right_col: String,
}

/// A CTE resolved to the base tables its body reads. A reference to the CTE
/// inlines them into the outer join graph: their tables, the join edges between
/// them, and a map from each CTE output column to the base columns that pass it
/// through. The body may join several base tables, read earlier CTEs, or be a
/// UNION (issue #60); a body that cannot be resolved (a subquery, INTERSECT /
/// EXCEPT) leaves the CTE unresolved, so a reference to it produces no cascade
/// path rather than a wrong one.
#[derive(Debug, Clone)]
struct ResolvedCte {
    tables: Vec<String>,
    edges: Vec<JoinEdge>,
    /// CTE output column → `(base_table, base_column)` for each branch that passes
    /// it through unchanged (aggregates / computed projections are absent).
    col_map: HashMap<String, Vec<(String, String)>>,
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

/// Add a FROM/JOIN table to the graph, inlining a CTE reference.
///
/// For a CTE reference its base tables and internal join edges join the graph,
/// and both the FROM alias and the CTE name are registered in `cte_refs` so column
/// references (`alias.col` or `cte.col`) can be remapped to the base columns in
/// `extract_col_ref`.
fn add_table_or_cte(
    name: &str,
    alias: Option<&str>,
    ctes: &HashMap<String, ResolvedCte>,
    graph: &mut JoinGraph,
) {
    if let Some(resolved) = ctes.get(name) {
        graph.tables.extend(resolved.tables.iter().cloned());
        for edge in &resolved.edges {
            graph.add_edge(edge.clone());
        }
        graph.cte_refs.insert(name.to_string(), resolved.clone());
        if let Some(a) = alias {
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
                // A CTE column can stand for several base columns (one per UNION
                // branch): connect every pair.
                let lefts = extract_col_ref(left, graph);
                let rights = extract_col_ref(right, graph);
                for (lt, lc) in &lefts {
                    for (rt, rc) in &rights {
                        if lt != rt {
                            graph.add_edge(JoinEdge {
                                left_table: lt.clone(),
                                left_col: lc.clone(),
                                right_table: rt.clone(),
                                right_col: rc.clone(),
                            });
                        }
                    }
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

/// Extract a `table.column` reference from an expression, resolving aliases, as
/// the `(table, column)` base columns it stands for.
///
/// A plain table column yields one entry. A CTE column yields the base columns it
/// passes through (one per UNION branch); a non-passthrough CTE output (aggregate
/// or computed, absent from the CTE's `col_map`) yields none, so no (wrong) edge
/// is created for it.
fn extract_col_ref(expr: &Expr, graph: &JoinGraph) -> Vec<(String, String)> {
    match expr {
        Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
            column_sources(&parts[0].value, &parts[1].value, graph)
        }
        Expr::Nested(inner) => extract_col_ref(inner, graph),
        _ => Vec::new(),
    }
}

/// The base columns behind `qualifier.column` (a table, an alias or a CTE).
fn column_sources(qualifier: &str, column: &str, graph: &JoinGraph) -> Vec<(String, String)> {
    if let Some(cte) = graph.cte_refs.get(qualifier) {
        return cte.col_map.get(column).cloned().unwrap_or_default();
    }
    vec![(graph.resolve(qualifier), column.to_string())]
}

/// Extract implicit joins from WHERE clause equality conditions
fn extract_implicit_joins(expr: &Expr, graph: &mut JoinGraph) {
    extract_equalities(expr, graph);
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

/// The join graph of a plain `SELECT … FROM … JOIN` (no CTE, no set operation),
/// with the SELECT itself. `None` for anything else.
fn plain_select_graph(select_sql: &str) -> Option<(Box<Select>, JoinGraph)> {
    let stmts = Parser::new(&PostgreSqlDialect {})
        .try_with_sql(select_sql)
        .ok()?
        .parse_statements()
        .ok()?;
    let Some(Statement::Query(query)) = stmts.into_iter().next() else {
        return None;
    };
    if query.with.is_some() {
        return None;
    }
    let SetExpr::Select(select) = *query.body else {
        return None;
    };
    let mut graph = JoinGraph::new();
    for twj in &select.from {
        build_graph_from_table_with_joins(twj, &mut graph, &HashMap::new()).ok()?;
    }
    if let Some(where_expr) = &select.selection {
        extract_implicit_joins(where_expr, &mut graph);
    }
    Some((select, graph))
}

/// The qualifier (its alias, or the table name when it has none) under which a
/// plain `SELECT` reads `table`. `None` when it reads it more than once or not at
/// all, or the query is not a plain `SELECT … FROM … JOIN`.
#[must_use]
pub fn table_qualifier(select_sql: &str, table: &str) -> Option<String> {
    let (select, _) = plain_select_graph(select_sql)?;
    let mut qualifiers = Vec::new();
    for twj in &select.from {
        let factors = std::iter::once(&twj.relation).chain(twj.joins.iter().map(|j| &j.relation));
        for factor in factors {
            let (name, alias) = extract_table_info(factor).ok()?;
            if name == table {
                qualifiers.push(alias.unwrap_or(name));
            }
        }
    }
    match qualifiers.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// The output column of a plain `SELECT` that projects `table.column` unchanged
/// (`t.col`, `t.col AS name`, or a bare `col`), if any.
#[must_use]
pub fn output_column_for(select_sql: &str, table: &str, column: &str) -> Option<String> {
    let (select, graph) = plain_select_graph(select_sql)?;
    projected_output(&select, &graph, table, column)
}

/// The output name of the projection item that is the column `table.column`.
fn projected_output(
    select: &Select,
    graph: &JoinGraph,
    table: &str,
    column: &str,
) -> Option<String> {
    select.projection.iter().find_map(|item| {
        let (expr, name) = match item {
            SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.value.clone())),
            SelectItem::UnnamedExpr(expr) => (expr, expr_bare_column(expr)),
            _ => return None,
        };
        let projects = match expr {
            Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
                graph.resolve(&parts[0].value) == table && parts[1].value == column
            }
            Expr::Identifier(ident) => ident.value == column,
            _ => false,
        };
        if projects { name } else { None }
    })
}

/// For each entity in `entities` whose relation (`v_<entity>` or `tv_<entity>`)
/// the definition reads, the output column carrying the value that relation is
/// joined to on `pk_<entity>`. A change to that entity's row `k` must refresh the
/// rows whose output column equals `k`.
///
/// The column is `None` when the definition reads the relation but no projected
/// column carries its key: the join is not an equality on `pk_<entity>`, the other
/// side is not projected, or the relation is read outside a plain `SELECT … FROM
/// … JOIN` (a subquery, a CTE, a set operation).
///
/// # Errors
///
/// Returns `Err` if the SQL cannot be parsed.
pub fn embed_lookup_columns(
    select_sql: &str,
    entities: &[String],
) -> Result<Vec<(String, Option<String>)>, String> {
    let referenced = referenced_entities(select_sql, entities)?;
    if referenced.is_empty() {
        return Ok(Vec::new());
    }

    // A definition that does not parse at all is rejected by the caller's own
    // parsing; here it only yields no lookup column.
    let parsed = plain_select_graph(select_sql);
    Ok(referenced
        .into_iter()
        .map(|entity| {
            let column = parsed
                .as_ref()
                .and_then(|(select, graph)| pk_join_output(select, graph, &entity));
            (entity, column)
        })
        .collect())
}

/// The entities among `entities` whose `v_<entity>` or `tv_<entity>` relation the
/// SQL names anywhere, in `entities` order.
fn referenced_entities(select_sql: &str, entities: &[String]) -> Result<Vec<String>, String> {
    use sqlparser::tokenizer::{Token, Tokenizer};
    let tokens = Tokenizer::new(&PostgreSqlDialect {}, select_sql)
        .tokenize()
        .map_err(|e| format!("SQL tokenize error: {e}"))?;
    let words: HashSet<String> = tokens
        .into_iter()
        .filter_map(|t| match t {
            Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_lowercase()),
            Token::Word(w) => Some(w.value),
            _ => None,
        })
        .collect();
    Ok(entities
        .iter()
        .filter(|e| words.contains(&format!("v_{e}")) || words.contains(&format!("tv_{e}")))
        .cloned()
        .collect())
}

/// The output column projecting the column that `v_<entity>` / `tv_<entity>` is
/// joined to on `pk_<entity>`.
fn pk_join_output(select: &Select, graph: &JoinGraph, entity: &str) -> Option<String> {
    let pk = format!("pk_{entity}");
    let relations = [format!("v_{entity}"), format!("tv_{entity}")];
    let is_pk = |table: &String, col: &String| relations.contains(table) && *col == pk;
    graph.edges.iter().find_map(|e| {
        let (table, col) = if is_pk(&e.left_table, &e.left_col) {
            (&e.right_table, &e.right_col)
        } else if is_pk(&e.right_table, &e.right_col) {
            (&e.left_table, &e.left_col)
        } else {
            return None;
        };
        projected_output(select, graph, table, col)
    })
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
    fn test_recursive_cte_detected() {
        assert!(has_recursive_cte(
            "WITH RECURSIVE t AS (SELECT 1) SELECT pk_x FROM tb_x"
        ));
        assert!(!has_recursive_cte("SELECT pk_x FROM tb_x"));
        assert!(!has_recursive_cte(
            "WITH t AS (SELECT 1) SELECT pk_x FROM tb_x"
        ));
    }

    fn aggregates() -> Vec<String> {
        vec!["user_summary".to_string(), "tag_count".to_string()]
    }

    #[test]
    fn embed_lookup_through_left_join_on_own_pk() {
        let sql = "SELECT u.pk_user, u.id, jsonb_build_object('s', s.data) AS data \
                   FROM tb_user u LEFT JOIN v_user_summary s ON s.pk_user_summary = u.pk_user";
        assert_eq!(
            embed_lookup_columns(sql, &aggregates()).unwrap(),
            vec![("user_summary".to_string(), Some("pk_user".to_string()))]
        );
    }

    #[test]
    fn embed_lookup_uses_the_output_alias_and_tv_relation() {
        let sql = "SELECT p.pk_post, p.fk_author AS author, c.data AS tags \
                   FROM tb_post p JOIN tv_tag_count c ON p.fk_author = c.pk_tag_count";
        assert_eq!(
            embed_lookup_columns(sql, &aggregates()).unwrap(),
            vec![("tag_count".to_string(), Some("author".to_string()))]
        );
    }

    #[test]
    fn embed_lookup_through_where_equality() {
        let sql = "SELECT u.pk_user, s.data FROM tb_user u, v_user_summary s \
                   WHERE u.pk_user = s.pk_user_summary";
        assert_eq!(
            embed_lookup_columns(sql, &aggregates()).unwrap(),
            vec![("user_summary".to_string(), Some("pk_user".to_string()))]
        );
    }

    #[test]
    fn embed_lookup_none_when_join_column_not_projected() {
        let sql = "SELECT p.pk_post, s.data FROM tb_post p \
                   JOIN v_user_summary s ON s.pk_user_summary = p.fk_user";
        assert_eq!(
            embed_lookup_columns(sql, &aggregates()).unwrap(),
            vec![("user_summary".to_string(), None)]
        );
    }

    #[test]
    fn embed_lookup_none_inside_a_subquery() {
        let sql = "SELECT u.pk_user, (SELECT s.data FROM v_user_summary s \
                   WHERE s.pk_user_summary = u.pk_user) AS data FROM tb_user u";
        assert_eq!(
            embed_lookup_columns(sql, &aggregates()).unwrap(),
            vec![("user_summary".to_string(), None)]
        );
    }

    #[test]
    fn embed_lookup_ignores_unreferenced_entities() {
        let sql = "SELECT u.pk_user, u.data FROM tb_user u JOIN v_user_summary_extra x \
                   ON x.pk_user = u.pk_user";
        assert!(embed_lookup_columns(sql, &aggregates()).unwrap().is_empty());
    }

    #[test]
    fn table_qualifier_is_the_alias_or_the_name() {
        let sql = "SELECT p.pk_post FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user";
        assert_eq!(table_qualifier(sql, "tb_user"), Some("u".to_string()));
        let sql = "SELECT pk_post FROM tb_post JOIN tb_user ON tb_user.pk_user = tb_post.fk_user";
        assert_eq!(table_qualifier(sql, "tb_user"), Some("tb_user".to_string()));
    }

    #[test]
    fn table_qualifier_none_when_read_twice_or_not_at_all() {
        let sql = "SELECT p.pk_post FROM tb_post p JOIN tb_user a ON a.pk_user = p.fk_author \
                   JOIN tb_user e ON e.pk_user = p.fk_editor";
        assert_eq!(table_qualifier(sql, "tb_user"), None);
        assert_eq!(table_qualifier(sql, "tb_tag"), None);
    }

    #[test]
    fn output_column_for_follows_aliases() {
        let sql = "SELECT p.pk_post, p.fk_user AS author, u.name FROM tb_post p \
                   JOIN tb_user u ON u.pk_user = p.fk_user";
        assert_eq!(
            output_column_for(sql, "tb_post", "fk_user"),
            Some("author".to_string())
        );
        assert_eq!(
            output_column_for(sql, "tb_user", "name"),
            Some("name".to_string())
        );
        assert_eq!(output_column_for(sql, "tb_user", "pk_user"), None);
    }
}

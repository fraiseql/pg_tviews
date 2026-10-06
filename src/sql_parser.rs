//! SQL text helpers for what registration reads from a definition's text rather
//! than its query tree: the output column that projects a table's column, and the
//! qualifier a table is read under (fan-out field maps, column-rename rewriting).

use sqlparser::ast::{Expr, Select, SelectItem, SetExpr, Statement, TableFactor};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use std::collections::HashMap;

/// The relations of a plain `SELECT`: alias → real table name.
#[derive(Debug, Default)]
struct JoinGraph {
    aliases: HashMap<String, String>,
}

impl JoinGraph {
    /// Resolve an identifier (alias or table name) to the real table name
    fn resolve(&self, name: &str) -> String {
        self.aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
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
        _ => Err("Subqueries and derived tables are not plain relations".to_string()),
    }
}

/// A plain `SELECT … FROM … JOIN` (no CTE, no set operation) with its relations.
/// `None` for anything else.
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
    let mut graph = JoinGraph::default();
    for twj in &select.from {
        let factors = std::iter::once(&twj.relation).chain(twj.joins.iter().map(|j| &j.relation));
        for factor in factors {
            let (name, alias) = extract_table_info(factor).ok()?;
            if let Some(alias) = alias {
                graph.aliases.insert(alias, name);
            }
        }
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

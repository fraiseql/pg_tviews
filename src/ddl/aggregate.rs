//! Aggregate TVIEWs (issue #58): an entity with no `tb_<entity>` whose rows are the
//! `GROUP BY` groups of its source tables.
//!
//! The caller names, for each source table, the column whose value **is** the group
//! key (`group_keys`). Each becomes a cascade path from that table straight to the
//! entity, so the ordinary row trigger enqueues the affected groups (both the old and
//! the new group when a row moves) and the ordinary pk refresh recomputes them from
//! the backing view: a new group is inserted, an emptied one deleted.

use crate::cascade_path::CascadePath;
use crate::error::{TViewError, TViewResult};
use pgrx::datum::DatumWithOid;
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use sqlparser::ast::{Expr, GroupByExpr, SelectItem, SetExpr, Statement};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::keywords::Keyword;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};
use std::collections::BTreeMap;

/// Source table name → the column whose value is the group key.
pub type GroupKeys = BTreeMap<String, String>;

/// Check that an aggregate definition can be maintained group by group.
///
/// Window functions are rejected (a window spans rows of other groups). The
/// `pk_<entity>` output must be a plain column that is also a `GROUP BY` key, so a
/// refresh's `WHERE pk_<entity> = ANY(…)` narrows the aggregate to the touched
/// groups instead of recomputing all of them.
///
/// # Errors
/// Returns the reason the definition is not supported.
pub fn validate_definition(sql: &str, entity: &str) -> Result<(), String> {
    let tokens = Tokenizer::new(&PostgreSqlDialect {}, sql)
        .tokenize()
        .map_err(|e| format!("cannot tokenize the definition: {e}"))?;
    if tokens
        .iter()
        .any(|t| matches!(t, Token::Word(w) if w.keyword == Keyword::OVER))
    {
        return Err(
            "window functions (OVER …) are not supported in aggregate TVIEWs: a window \
                    spans rows of other groups, so a group cannot be recomputed on its own"
                .to_string(),
        );
    }

    let stmt = Parser::parse_sql(&PostgreSqlDialect {}, sql)
        .map_err(|e| format!("cannot parse the definition: {e}"))?
        .into_iter()
        .next()
        .ok_or("empty definition")?;
    let Statement::Query(query) = stmt else {
        return Err("the definition must be a SELECT".to_string());
    };
    let SetExpr::Select(select) = &*query.body else {
        return Err("the definition must be a single SELECT with GROUP BY (no UNION)".to_string());
    };

    let pk_col = format!("pk_{entity}");
    let pk_expr = select
        .projection
        .iter()
        .find_map(|item| match item {
            SelectItem::ExprWithAlias { expr, alias } if alias.value == pk_col => Some(expr),
            SelectItem::UnnamedExpr(e @ Expr::Identifier(i)) if i.value == pk_col => Some(e),
            SelectItem::UnnamedExpr(e @ Expr::CompoundIdentifier(p))
                if p.last().is_some_and(|i| i.value == pk_col) =>
            {
                Some(e)
            }
            _ => None,
        })
        .ok_or_else(|| format!("the definition must output a {pk_col} column (the group key)"))?;
    if !matches!(pk_expr, Expr::Identifier(_) | Expr::CompoundIdentifier(_)) {
        return Err(format!(
            "{pk_col} must be a plain column that is also a GROUP BY key, not an expression"
        ));
    }
    let GroupByExpr::Expressions(keys, _) = &select.group_by else {
        return Err("the definition must have a GROUP BY".to_string());
    };
    if !keys.iter().any(|k| k.to_string() == pk_expr.to_string()) {
        return Err(format!(
            "{pk_col} ({pk_expr}) must also be a GROUP BY key, so a refresh recomputes only \
             the touched groups"
        ));
    }
    Ok(())
}

/// One cascade path per group key: a change to a row of `table` refreshes the
/// group named by that row's `column`.
///
/// # Errors
/// Returns an error if a named table is not a source of the view or lacks the column.
pub fn cascade_paths(
    entity: &str,
    group_keys: &GroupKeys,
    base_tables: &[Oid],
    view_oid: Oid,
) -> TViewResult<Vec<CascadePath>> {
    let mut paths = Vec::with_capacity(group_keys.len());
    for (table, column) in group_keys {
        let oid =
            source_table_oid(table, base_tables)?.ok_or_else(|| TViewError::InvalidInput {
                parameter: "group_keys".to_string(),
                reason: format!("'{table}' is not a table the definition reads"),
            })?;
        if !column_exists(oid, column)? {
            return Err(TViewError::InvalidInput {
                parameter: "group_keys".to_string(),
                reason: format!("table '{table}' has no column '{column}'"),
            });
        }
        paths.push(CascadePath {
            source_oid: oid,
            source_table: table.clone(),
            entity_name: entity.to_string(),
            initial_col: column.clone(),
            hops: Vec::new(),
            unresolvable: false,
            source_columns: crate::ddl::create::view_source_columns(view_oid, oid),
            fanout: None,
            root: false,
            initial_attnum: None,
        });
    }
    Ok(paths)
}

fn source_table_oid(table: &str, base_tables: &[Oid]) -> TViewResult<Option<Oid>> {
    let args = [
        unsafe { DatumWithOid::new(table, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        unsafe {
            DatumWithOid::new(
                base_tables.to_vec(),
                PgOid::BuiltIn(PgBuiltInOids::OIDARRAYOID).value(),
            )
        },
    ];
    Spi::connect(|client| {
        let mut rows = client.select(
            "SELECT oid FROM pg_class WHERE relname = $1 AND oid = ANY($2)",
            Some(1),
            &args,
        )?;
        match rows.next() {
            Some(row) => row.get::<Oid>(1),
            None => Ok(None),
        }
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Resolve group key table '{table}'"),
        pg_error: e.to_string(),
    })
}

fn column_exists(table: Oid, column: &str) -> TViewResult<bool> {
    let args = [
        unsafe { DatumWithOid::new(table, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) },
        unsafe { DatumWithOid::new(column, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
    ];
    Spi::connect(|client| {
        client
            .select(
                "SELECT EXISTS (SELECT 1 FROM pg_attribute \
                 WHERE attrelid = $1 AND attname = $2 AND attnum > 0 AND NOT attisdropped)",
                Some(1),
                &args,
            )?
            .first()
            .get_one::<bool>()
    })
    .map(|b| b.unwrap_or(false))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check group key column '{column}'"),
        pg_error: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::validate_definition as v;

    const OK: &str = "SELECT o.fk_user AS pk_user_summary, u.id, \
                      jsonb_build_object('orders', count(*)) AS data \
                      FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user \
                      GROUP BY o.fk_user, u.id";

    #[test]
    fn accepts_a_grouped_plain_key() {
        assert_eq!(v(OK, "user_summary"), Ok(()));
    }

    #[test]
    fn rejects_window_functions() {
        let sql = "SELECT o.fk_user AS pk_user_summary, u.id, \
                   jsonb_build_object('rank', row_number() OVER (ORDER BY o.fk_user)) AS data \
                   FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user GROUP BY o.fk_user, u.id";
        assert!(
            v(sql, "user_summary")
                .unwrap_err()
                .contains("window functions")
        );
    }

    #[test]
    fn ignores_over_inside_a_string() {
        let sql = OK.replace("'orders'", "'over'");
        assert_eq!(v(&sql, "user_summary"), Ok(()));
    }

    #[test]
    fn rejects_an_expression_key_or_one_missing_from_group_by() {
        let expr = OK.replace(
            "o.fk_user AS pk_user_summary",
            "o.fk_user + 0 AS pk_user_summary",
        );
        assert!(
            v(&expr, "user_summary")
                .unwrap_err()
                .contains("plain column")
        );
        let ungrouped = OK.replace("GROUP BY o.fk_user, u.id", "GROUP BY u.id");
        assert!(
            v(&ungrouped, "user_summary")
                .unwrap_err()
                .contains("GROUP BY key")
        );
        assert!(v(OK, "other").unwrap_err().contains("pk_other"));
    }
}

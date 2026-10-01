//! Base tables a TVIEW reads whose writes no cascade maps to its keys (issues
//! #157, #158).
//!
//! The triggers go on every table the backing view reads (`pg_depend`), but a
//! write only refreshes the TVIEW when something maps the changed row to its keys:
//! the TVIEW's own `tb_<entity>`, a cascade path, or entity propagation from an
//! embedded TVIEW's view (`fk_<child>` or an aggregate embed). A table none of
//! them reaches is reported, and `pg_tviews.uncascaded_policy` decides what
//! happens to it.

use crate::cascade_path::CascadePath;
use crate::config::UncascadedPolicy;
use crate::error::{TViewError, TViewResult};
use pgrx::datum::DatumWithOid;
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;
use std::collections::HashSet;

/// A base table no cascade reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UncascadedTable {
    pub oid: Oid,
    /// Schema-qualified, quoted name.
    pub name: String,
    /// How the view reads it, when known.
    pub reason: String,
}

/// How the trigger and the flush learn what a TVIEW reached: the tables and the
/// stored policy.
#[derive(Debug, Clone)]
pub(crate) struct Uncascaded {
    pub tables: Vec<UncascadedTable>,
    pub policy: UncascadedPolicy,
}

impl Uncascaded {
    pub(crate) fn oids(&self) -> Vec<Oid> {
        self.tables.iter().map(|t| t.oid).collect()
    }
}

/// What a TVIEW embeds by entity propagation: the children whose changes refresh it.
pub(crate) struct Propagation<'a> {
    pub fk_columns: &'a [String],
    pub aggregate_entities: &'a [String],
}

impl Propagation<'_> {
    fn embeds(&self, child: &str) -> bool {
        self.fk_columns.iter().any(|c| c == &format!("fk_{child}"))
            || self.aggregate_entities.iter().any(|a| a == child)
    }
}

/// The base tables of `entity`'s backing view `view_oid` that neither its
/// `tb_<entity>`, nor a cascade path, nor propagation from an embedded TVIEW reaches.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub(crate) fn find(
    entity: &str,
    view_oid: Oid,
    base_tables: &[Oid],
    cascade_paths: &[CascadePath],
    propagation: &Propagation<'_>,
) -> TViewResult<Vec<UncascadedTable>> {
    if base_tables.is_empty() {
        return Ok(Vec::new());
    }
    let catalog = |e: spi::Error| TViewError::CatalogError {
        operation: format!("Find the base tables no cascade of tv_{entity} reaches"),
        pg_error: e.to_string(),
    };
    // Every way the view reaches each base table, with the first view on the way.
    let sql = "WITH RECURSIVE reads(relid, via_view) AS ( \
             SELECT $1::pg_catalog.oid, NULL::pg_catalog.oid \
           UNION \
             SELECT d.refobjid, \
                    COALESCE(r.via_view, CASE WHEN c.relkind = 'v' THEN d.refobjid END) \
             FROM reads r \
             JOIN pg_catalog.pg_class v ON v.oid = r.relid AND v.relkind = 'v' \
             JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid \
             JOIN pg_catalog.pg_depend d \
               ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass \
              AND d.objid = w.oid \
              AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
              AND d.refobjid <> v.oid \
             JOIN pg_catalog.pg_class c ON c.oid = d.refobjid \
         ) \
         SELECT r.relid, c.relname::pg_catalog.text, \
                pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname), \
                (SELECT pg_catalog.quote_ident(vn.nspname) || '.' || pg_catalog.quote_ident(vc.relname) \
                 FROM pg_catalog.pg_class vc \
                 JOIN pg_catalog.pg_namespace vn ON vn.oid = vc.relnamespace \
                 WHERE vc.oid = r.via_view) \
         FROM reads r \
         JOIN pg_catalog.pg_class c ON c.oid = r.relid \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE r.relid = ANY($2) \
         ORDER BY 3, 4 NULLS FIRST";
    // SAFETY: each datum borrows a value that outlives the select.
    let args = unsafe {
        [
            DatumWithOid::new(view_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()),
            DatumWithOid::new(
                base_tables.to_vec(),
                PgOid::BuiltIn(PgBuiltInOids::OIDARRAYOID).value(),
            ),
        ]
    };
    let routes = Spi::connect(|client| {
        let mut routes = Vec::new();
        for row in client.select(sql, None, &args)? {
            routes.push(Route {
                oid: row.get::<Oid>(1)?.unwrap_or(Oid::INVALID),
                relname: row.get::<String>(2)?.unwrap_or_default(),
                name: row.get::<String>(3)?.unwrap_or_default(),
                via_view: row.get::<String>(4)?,
            });
        }
        Ok::<_, spi::Error>(routes)
    })
    .map_err(catalog)?;
    let propagated = propagated_tables(entity, propagation).map_err(catalog)?;
    Ok(unreached(entity, &routes, cascade_paths, &propagated))
}

/// The tables whose writes refresh a TVIEW this one embeds (`fk_<child>` or an
/// aggregate embed), followed through the children's own children: entity
/// propagation then refreshes this TVIEW's rows that embed the changed child rows.
fn propagated_tables(entity: &str, propagation: &Propagation<'_>) -> spi::Result<HashSet<Oid>> {
    let metas = crate::catalog::TviewMeta::load_all()?;
    // `tb_<entity>` of each registered entity, as its backing view reads it.
    let roots: Vec<(String, Oid)> = Spi::connect(|client| {
        let mut roots = Vec::new();
        for row in client.select(
            &format!(
                "SELECT r.entity::pg_catalog.text, r.relid FROM {}.pg_tview_reads r \
                 JOIN pg_catalog.pg_class c ON c.oid = r.relid \
                 WHERE c.relname = 'tb_' || r.entity",
                crate::utils::ext_schema()
            ),
            None,
            &[],
        )? {
            if let (Some(e), Some(oid)) = (row.get::<String>(1)?, row.get::<Oid>(2)?) {
                roots.push((e, oid));
            }
        }
        Ok::<_, spi::Error>(roots)
    })?;
    let mut tables = HashSet::new();
    let mut seen: HashSet<&str> = HashSet::from([entity]);
    let mut children: Vec<&crate::catalog::TviewMeta> = metas
        .iter()
        .filter(|m| propagation.embeds(&m.entity_name))
        .collect();
    while let Some(child) = children.pop() {
        if !seen.insert(&child.entity_name) {
            continue;
        }
        tables.extend(
            roots
                .iter()
                .filter(|(e, _)| e == &child.entity_name)
                .map(|(_, oid)| *oid),
        );
        tables.extend(
            child
                .cascade_paths
                .iter()
                .filter(|p| !p.unresolvable)
                .map(|p| p.source_oid),
        );
        if child.uncascaded_policy == UncascadedPolicy::FullRefresh {
            tables.extend(child.uncascaded_oids.iter().copied());
        }
        children.extend(metas.iter().filter(|m| {
            child
                .fk_columns
                .iter()
                .any(|c| c == &format!("fk_{}", m.entity_name))
        }));
    }
    Ok(tables)
}

/// One way the backing view reaches a base table.
struct Route {
    oid: Oid,
    relname: String,
    name: String,
    via_view: Option<String>,
}

/// The tables of `routes` that none of the root, a cascade path or propagation
/// reaches, in order, each once.
fn unreached(
    entity: &str,
    routes: &[Route],
    cascade_paths: &[CascadePath],
    propagated: &HashSet<Oid>,
) -> Vec<UncascadedTable> {
    let root = format!("tb_{entity}");
    let mut tables: Vec<UncascadedTable> = Vec::new();
    for route in routes {
        let reached = route.relname == root
            || propagated.contains(&route.oid)
            || cascade_paths
                .iter()
                .any(|p| !p.unresolvable && p.source_oid == route.oid);
        if reached || tables.iter().any(|t| t.oid == route.oid) {
            continue;
        }
        // A table's direct route sorts first: name a view only when every route uses one.
        let reason = match &route.via_view {
            Some(view) => format!("read through view {view}"),
            None => "read in a subquery, or through a join pg_tviews cannot map".to_string(),
        };
        tables.push(UncascadedTable {
            oid: route.oid,
            name: route.name.clone(),
            reason,
        });
    }
    tables
}

/// `writes to a, b will not refresh tv (a: reason; b: reason)`.
fn describe(tview: &str, tables: &[UncascadedTable], verb: &str) -> String {
    let names = tables
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let reasons = match tables {
        [one] => one.reason.clone(),
        _ => tables
            .iter()
            .map(|t| format!("{}: {}", t.name, t.reason))
            .collect::<Vec<_>>()
            .join("; "),
    };
    format!("writes to {names} {verb} {tview} ({reasons})")
}

/// Report the uncascaded tables of `tview` (schema-qualified) under `policy`:
/// a WARNING, a NOTICE, or an ERROR that aborts the create.
///
/// # Errors
/// Returns an error under the `error` policy when `tables` is not empty.
pub(crate) fn report(
    tview: &str,
    tables: &[UncascadedTable],
    policy: UncascadedPolicy,
) -> TViewResult<()> {
    if tables.is_empty() {
        return Ok(());
    }
    match policy {
        UncascadedPolicy::Warn => {
            pg_sys::panic::ErrorReport::new(
                PgSqlErrorCode::ERRCODE_WARNING,
                describe(tview, tables, "will not refresh"),
                function_name!(),
            )
            .set_hint(
                "Set pg_tviews.uncascaded_policy to 'full_refresh' before creating the TVIEW \
                 to refresh it in full on such writes, or to 'error' to refuse it.",
            )
            .report(PgLogLevel::WARNING);
            Ok(())
        }
        UncascadedPolicy::FullRefresh => {
            notice!("{}", describe(tview, tables, "will refresh all rows of"));
            Ok(())
        }
        UncascadedPolicy::Error => Err(TViewError::InvalidInput {
            parameter: "tview definition".to_string(),
            reason: format!(
                "{}. Set pg_tviews.uncascaded_policy to 'warn' or 'full_refresh' to create it anyway",
                describe(tview, tables, "would not refresh")
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(oid: u32, relname: &str, via_view: Option<&str>) -> Route {
        Route {
            oid: Oid::from(oid),
            relname: relname.to_string(),
            name: format!("public.{relname}"),
            via_view: via_view.map(str::to_string),
        }
    }

    fn path(source: u32) -> CascadePath {
        CascadePath {
            source_oid: Oid::from(source),
            source_table: String::new(),
            entity_name: "order".to_string(),
            initial_col: "fk_order".to_string(),
            hops: vec![],
            unresolvable: false,
            source_columns: vec![],
            fanout: None,
        }
    }

    #[test]
    fn root_and_cascade_sources_are_reached() {
        let routes = [route(1, "tb_order", None), route(2, "tb_line", None)];
        assert!(unreached("order", &routes, &[path(2)], &HashSet::new()).is_empty());
    }

    #[test]
    fn a_table_without_path_is_named_once() {
        let routes = [
            route(1, "tb_order", None),
            route(2, "tb_line", None),
            route(2, "tb_line", Some("public.v_lines")),
        ];
        let got = unreached("order", &routes, &[], &HashSet::new());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "public.tb_line");
        assert!(got[0].reason.contains("subquery"));
    }

    #[test]
    fn a_table_under_a_view_names_the_view() {
        let routes = [route(2, "tb_line", Some("public.v_lines"))];
        let got = unreached("order", &routes, &[], &HashSet::new());
        assert_eq!(got[0].reason, "read through view public.v_lines");
    }

    #[test]
    fn propagation_reaches_an_embedded_tview_s_tables() {
        let routes = [route(3, "tb_user", Some("public.v_user"))];
        assert!(unreached("post", &routes, &[], &HashSet::from([Oid::from(3)])).is_empty());
        assert_eq!(unreached("post", &routes, &[], &HashSet::new()).len(), 1);
    }

    #[test]
    fn an_unresolvable_path_does_not_reach() {
        let mut p = path(2);
        p.unresolvable = true;
        let routes = [route(2, "tb_line", None)];
        assert_eq!(unreached("order", &routes, &[p], &HashSet::new()).len(), 1);
    }

    #[test]
    fn describe_lists_tables_and_reasons() {
        let t = |n: &str, r: &str| UncascadedTable {
            oid: Oid::INVALID,
            name: n.to_string(),
            reason: r.to_string(),
        };
        assert_eq!(
            describe("public.tv_o", &[t("public.a", "x")], "will not refresh"),
            "writes to public.a will not refresh public.tv_o (x)"
        );
        assert_eq!(
            describe(
                "public.tv_o",
                &[t("public.a", "x"), t("public.b", "y")],
                "will not refresh"
            ),
            "writes to public.a, public.b will not refresh public.tv_o (public.a: x; public.b: y)"
        );
    }
}

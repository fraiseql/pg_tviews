//! Functions acting on one TVIEW, named by `tview`.

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

use crate::ddl::replace;
use crate::error::TViewError;

/// Create a TVIEW from `query`, with `options` (ADR 0220). An existing one is an
/// error; [`pg_tviews_create_or_replace`] changes it.
///
/// Usage: `SELECT tviews.pg_tviews_create('tv_post', $$SELECT pk_post, id, … AS data FROM tb_post$$);`
#[pg_extern(name = "pg_tviews_create")]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx #[pg_extern] requires JsonB by value
fn pg_tviews_create_with_options(
    tview: &str,
    query: &str,
    options: default!(pgrx::JsonB, "'{}'"),
) -> Result<String, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let options = replace::parse_options(&options.0)?;
    match replace::create_only(tview, query, options, false) {
        Ok(replace::Created::Rows(_) | replace::Created::Skipped) => {
            Ok(format!("TVIEW {} created", created_relation(tview)))
        }
        Ok(replace::Created::Exists(name)) => Err(TViewError::RelationExists { name }.into()),
        Err(e) => Err(e.report_in("Failed to create TVIEW")),
    }
}

/// Create a TVIEW, or bring an existing one to `query` and `options` with the
/// smallest change: `created`, `unchanged`, `altered`, `replaced` or `rebuilt`.
/// The options are the whole declaration: an option not passed is at its default.
///
/// Usage: `SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$SELECT …$$,
/// options => '{"fillfactor": 90}');`
#[pg_extern]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx #[pg_extern] requires JsonB by value
fn pg_tviews_create_or_replace(
    tview: &str,
    query: &str,
    options: default!(pgrx::JsonB, "'{}'"),
) -> Result<String, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    replace::create_or_replace(tview, query, &options.0)
        .map(str::to_string)
        .map_err(|e| e.report_in(&format!("Failed to create or replace TVIEW {tview}")))
}

/// Drop a TVIEW: its table, its backing view, its triggers and its registration.
///
/// Usage: `SELECT tviews.pg_tviews_drop('post', if_exists => true);`
#[pg_extern]
fn pg_tviews_drop(
    tview: &str,
    if_exists: default!(bool, false),
    cascade: default!(bool, false),
) -> Result<String, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let relation = crate::catalog::resolve::find(tview)
        .ok()
        .map(|found| super::table_name(found.table, &found.entity));
    match crate::ddl::drop_tview(tview, if_exists, cascade) {
        Ok(true) => Ok(format!(
            "TVIEW {} dropped",
            relation.unwrap_or_else(|| tview.to_string())
        )),
        Ok(false) => Ok(format!("TVIEW {tview} does not exist, nothing dropped")),
        Err(e) => Err(e.report_in("Failed to drop TVIEW")),
    }
}

/// Re-derive a TVIEW's registration and triggers from its stored definition with
/// this release's analysis, and clear `needs_reregister`. Its rows are not
/// touched.
#[pg_extern]
fn pg_tviews_reregister(tview: &str) -> Result<String, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let found = super::owned(tview)?;
    crate::ddl::create::reregister_tview(&found.entity)
        .map(|()| "reregistered".to_string())
        .map_err(|e| e.report_in(&format!("Failed to re-register TVIEW {tview}")))
}

/// Rebuild a TVIEW from its backing view, then every TVIEW reading it, in
/// dependency order, each as its owner.
#[pg_extern]
fn pg_tviews_refresh(tview: &str) -> Result<(), ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let found = super::owned(tview)?;
    crate::admin::rebuild_with_dependents(&[found.entity])?;
    Ok(())
}

/// Bring the TVIEWs whose definitions read the current time up to date: `tview`,
/// or every such TVIEW the caller owns. Returns the TVIEWs refreshed,
/// dependencies first.
#[pg_extern]
fn pg_tviews_refresh_time_dependent(
    tview: default!(Option<&str>, "NULL"),
) -> Result<SetOfIterator<'static, String>, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let chosen = match tview {
        Some(name) => {
            let meta = super::owned_meta(name)?;
            if !meta.time_dependent {
                return Err(TViewError::InvalidInput {
                    parameter: "tview".to_string(),
                    reason: format!(
                        "{} does not read the time: nothing to refresh",
                        super::relation(&meta)
                    ),
                }
                .into());
            }
            vec![meta]
        }
        None => crate::admin::owned_time_dependent()?,
    };
    Ok(SetOfIterator::new(crate::admin::refresh_time_dependent(
        &chosen,
    )?))
}

/// Create the propagation indexes `(<lookup>, <identity>)` a TVIEW (or every
/// TVIEW, the caller owning each) is missing, and return their DDL; with
/// `dry_run`, only return it.
#[pg_extern]
fn pg_tviews_ensure_propagation_indexes(
    tview: default!(Option<&str>, "NULL"),
    dry_run: default!(bool, false),
) -> Result<SetOfIterator<'static, String>, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let metas = if let Some(name) = tview {
        vec![super::owned_meta(name)?]
    } else {
        let metas: Vec<_> = crate::catalog::TviewMeta::load_all()?
            .into_iter()
            .map(std::rc::Rc::new)
            .collect();
        for meta in &metas {
            crate::owner::require_owner(meta.tview_oid, &super::relation(meta))?;
        }
        metas
    };
    Ok(SetOfIterator::new(
        crate::admin::ensure_propagation_indexes(&metas, dry_run)?,
    ))
}

/// The TVIEWs that embed `tview`, transitively: each with its depth and the TVIEW
/// it embeds.
#[pg_extern]
#[allow(clippy::type_complexity)] // Reason: pgrx TableIterator row type spells out the columns
fn pg_tviews_show_cascade_path(
    tview: &str,
) -> Result<
    TableIterator<
        'static,
        (
            name!(depth, i32),
            name!(entity, String),
            name!(depends_on, String),
        ),
    >,
    ErrorReport,
> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let found = crate::catalog::resolve::find(tview)?;
    Ok(TableIterator::new(crate::admin::cascade_path(
        &found.entity,
    )?))
}

/// The query that maps changed rows of `base_table`, read from a relation named
/// `pg_tviews_delta`, to keys of `tview` (ADR 0157). NULL when writes to the table
/// do not map through a query of their own (`propagated`, `all_keys`) or the
/// TVIEW does not read it.
#[pg_extern]
fn pg_tviews_mapping_query(
    tview: &str,
    base_table: pg_sys::Oid,
) -> Result<Option<String>, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let report = |e: TViewError| e.report_in(&format!("pg_tviews: the mapping query of {tview}"));
    let meta = crate::catalog::resolve::resolve(tview)?;
    crate::delta::mapping_query(&meta, base_table).map_err(report)
}

/// What a refresh of `tview`'s rows reads of `base_table`, for value locks (ADR
/// 0207): for each column its mapping joins on (NULL when the table is locked as a
/// whole), the query from the TVIEW's keys (`$1`) to the values a refresh locks.
#[pg_extern]
#[allow(clippy::type_complexity)] // Reason: pgrx TableIterator row type spells out the columns
fn pg_tviews_read_set_queries(
    tview: &str,
    base_table: pg_sys::Oid,
) -> Result<
    TableIterator<
        'static,
        (
            name!(column_name, Option<String>),
            name!(query, Option<String>),
        ),
    >,
    ErrorReport,
> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let report = |e: TViewError| e.report_in(&format!("pg_tviews: the read sets of {tview}"));
    let meta = crate::catalog::resolve::resolve(tview)?;
    Ok(TableIterator::new(
        crate::delta::read_set_queries(&meta, base_table).map_err(report)?,
    ))
}

/// The table a create made of `tview`, for its message.
fn created_relation(tview: &str) -> String {
    crate::catalog::resolve::find(tview).map_or_else(
        |_| tview.to_string(),
        |found| super::table_name(found.table, &found.entity),
    )
}

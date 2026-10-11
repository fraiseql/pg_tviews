//! A TVIEW's rows computed from its backing view as a whole: the rebuild
//! (`TRUNCATE` and fill), the refill of a reset TVIEW (without `TRUNCATE`), the
//! fill of an empty one, and the reconcile that touches only rows that change.
//! Each runs as the TVIEW's owner, and refuses a key its view returns twice.

use crate::catalog::TviewMeta;
use crate::error::{TViewError, TViewResult};
use crate::utils::ident;
use pgrx::prelude::*;
use pgrx::spi;

/// Rebuild one TVIEW from its backing view (`TRUNCATE` + `INSERT … SELECT`), as
/// its owner, and nothing that reads it.
///
/// The backing view runs the functions it calls as the querying role: rebuilding
/// as the caller would run the owner's view code with the caller's privileges
/// (a superuser's, after a migration).
///
/// # Errors
/// Returns error if the entity is not registered or the truncate/insert fails.
pub fn rebuild_one(entity: &str) -> TViewResult<()> {
    crate::stats::add(entity, crate::stats::Counter::FullRefreshes, 1);
    if let Some(meta) = crate::catalog::TviewMeta::load_by_entity(entity)? {
        // Every row changes: refreshes of any of them wait, and are waited for,
        // and so are writers of anything they read.
        crate::concurrency::lock_relation(
            meta.tview_oid.to_u32(),
            crate::concurrency::Side::Writer,
        );
        crate::concurrency::reads::lock_whole_read_set(&meta)?;
    }
    let owner = crate::owner::AsOwner::of_entity(entity)?;
    let (qi_tv, insert) = rebuild_statements(&owner, entity)?;
    Spi::run(&format!("TRUNCATE {qi_tv}"))?;
    Spi::run(&insert)?;
    drop(owner);
    // Rebuilt from its view: its rows can be trusted again.
    let meta = crate::catalog::TviewMeta::load_by_entity(entity)?.ok_or_else(|| {
        TViewError::TviewNotFound {
            name: entity.to_string(),
        }
    })?;
    crate::lifecycle::validity::mark(meta.tview_oid)?;
    Ok(())
}

/// Replace every row of `tv_<entity>` with its backing view's, without `TRUNCATE`
/// (readers are never blocked), as its owner: the fill of a TVIEW whose rows
/// can't be trusted ([`crate::lifecycle::validity`]). Rows a concurrent writer
/// committed meanwhile are kept.
///
/// # Errors
/// Returns error if the entity is not registered or the delete/insert fails.
pub fn refill(entity: &str) -> TViewResult<()> {
    crate::stats::add(entity, crate::stats::Counter::FullRefreshes, 1);
    if let Some(meta) = crate::catalog::TviewMeta::load_by_entity(entity)? {
        crate::concurrency::reads::lock_whole_read_set(&meta)?;
    }
    let owner = crate::owner::AsOwner::of_entity(entity)?;
    let (qi_tv, insert) = rebuild_statements(&owner, entity)?;
    Spi::run(&format!("DELETE FROM {qi_tv}"))?;
    Spi::run(&format!("{insert} ON CONFLICT DO NOTHING"))?;
    Ok(())
}

/// Populate an **empty** `tv_<entity>` from its backing view without `TRUNCATE`,
/// as its owner.
///
/// Used to fill a TVIEW created empty (a rebuild by another role). Unlike
/// [`rebuild_one`] it takes only a ROW EXCLUSIVE lock, so readers are never
/// blocked, even when the transaction stays prepared (2PC) for a while.
///
/// # Errors
/// Returns error if the entity is not registered or the insert fails.
pub fn fill_empty_tview(entity: &str) -> TViewResult<()> {
    let owner = crate::owner::AsOwner::of_entity(entity)?;
    let (_, insert) = rebuild_statements(&owner, entity)?;
    Spi::run(&insert)?;
    Ok(())
}

/// The schema-qualified TVIEW table and the `INSERT … SELECT` that fills it from
/// its backing view, once the view is known to return one row per key. The
/// explicit column list comes from the view's own columns, which excludes the
/// table-only `created_at`/`updated_at` columns.
/// Taking the owner's guard makes running the statements as anyone else
/// unrepresentable.
fn rebuild_statements(
    _owner: &crate::owner::AsOwner,
    entity: &str,
) -> TViewResult<(String, String)> {
    use crate::catalog::TviewMeta;

    let meta = TviewMeta::load_by_entity(entity)?.ok_or_else(|| TViewError::TviewNotFound {
        name: entity.to_string(),
    })?;
    let qi_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qi_view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
    let view_columns = crate::utils::get_view_columns_by_oid(meta.view_oid)?;
    if view_columns.is_empty() {
        return Err(TViewError::CatalogError {
            operation: format!("Get columns for view {qi_view}"),
            pg_error: "View has no selectable columns".to_string(),
        });
    }
    let col_list = view_columns
        .iter()
        .map(|c| ident::quoted(c))
        .collect::<Vec<_>>()
        .join(", ");
    // A key names one row: a UNION view returning several for one is refused
    // before the fill, instead of failing on the table's primary key.
    crate::refresh::refuse_duplicate_keys(&meta, "true", &[])?;
    let insert = format!("INSERT INTO {qi_tv} ({col_list}) SELECT {col_list} FROM {qi_view}");
    Ok((qi_tv, insert))
}

/// Bring the rows of a TVIEW's table to those of its backing view with three
/// statements that touch only rows that change, journaling each change. Rows
/// that leave go first, so a unique index holds throughout. Returns the
/// `pk_<entity>` of every row deleted, updated or inserted.
pub(crate) fn reconcile(entity: &str, meta: &TviewMeta) -> TViewResult<Vec<String>> {
    use crate::queue::affected::{Change, record};
    crate::stats::add(entity, crate::stats::Counter::FullRefreshes, 1);
    let _pin = crate::owner::RenderPin::new();
    // Every row is computed: writers of anything they read wait, and are waited for.
    crate::concurrency::reads::lock_whole_read_set(meta)?;

    // A key names one row: a UNION view returning several for one is refused.
    crate::refresh::refuse_duplicate_keys(meta, "true", &[])?;
    let qualified_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qualified_view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;

    let columns = crate::utils::get_view_columns_by_oid(meta.view_oid)?;
    let keys = crate::utils::spi::strings(
        "SELECT a.attname::text FROM pg_catalog.pg_index i \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid \
          AND a.attnum = ANY (i.indkey) \
         WHERE i.indrelid = $1 AND i.indisprimary ORDER BY a.attnum",
        &[crate::utils::spi::oid(meta.tview_oid)],
    )?;
    let prefixed = |columns: &[&String], prefix: &str| -> Vec<String> {
        columns
            .iter()
            .map(|c| format!("{prefix}{}", ident::quoted(c)))
            .collect()
    };
    let list = |columns: &[&String], prefix: &str| prefixed(columns, prefix).join(", ");
    let key_columns: Vec<&String> = keys.iter().collect();
    let value_columns: Vec<&String> = columns.iter().filter(|c| !keys.contains(c)).collect();
    let all_columns: Vec<&String> = columns.iter().collect();
    let same_key = format!(
        "({}) = ({})",
        list(&key_columns, "t."),
        list(&key_columns, "v.")
    );
    let pk = ident::quoted(&format!("pk_{entity}"));

    let deleted = Spi::connect_mut(|client| {
        let mut rows = Vec::new();
        for row in client.update(
            &format!(
                "DELETE FROM {qualified_tv} t \
                 WHERE NOT EXISTS (SELECT 1 FROM {qualified_view} v WHERE {same_key}) \
                 RETURNING t.{pk}::text, pg_catalog.to_jsonb(t.*)->>'id'"
            ),
            None,
            &[],
        )? {
            if let Some(key) = row.get::<String>(1)? {
                rows.push((key, row.get::<String>(2)?));
            }
        }
        Ok::<_, spi::Error>(rows)
    })
    .map_err(|e| {
        crate::utils::spi::catalog_error("Delete the rows the new definition drops", &e)
    })?;
    let mut changed: Vec<String> = deleted.iter().map(|(key, _)| key.clone()).collect();
    for (key, id) in deleted {
        record(entity, key, Change::Deleted(id));
    }
    if !value_columns.is_empty() {
        // A stored column can have another type than the view's (an unmapped user type
        // is stored as text): compare against the value the UPDATE would assign.
        let stored: std::collections::HashMap<String, String> =
            crate::utils::column_types(meta.tview_oid)?
                .into_iter()
                .collect();
        let stored_types: Vec<String> = value_columns
            .iter()
            .filter_map(|c| stored.get(c.as_str()).cloned())
            .collect();
        if stored_types.len() != value_columns.len() {
            return Err(TViewError::CatalogError {
                operation: format!("Compare the rows of {qualified_tv} with its view"),
                pg_error: "a view column is missing from the TVIEW's table".to_string(),
            });
        }
        let fresh: Vec<String> = prefixed(&value_columns, "v.")
            .into_iter()
            .zip(&stored_types)
            .map(|(v, ty)| format!("{v}::{ty}"))
            .collect();
        let set = value_columns
            .iter()
            .map(|c| format!("{0} = v.{0}", ident::quoted(c)))
            .collect::<Vec<_>>()
            .join(", ");
        for key in crate::utils::spi::strings(
            &format!(
                "UPDATE {qualified_tv} t SET {set}, updated_at = pg_catalog.now() \
                 FROM {qualified_view} v \
                 WHERE {same_key} AND {} \
                 RETURNING t.{pk}::text",
                crate::refresh::rows_differ(&prefixed(&value_columns, "t."), &fresh)
            ),
            &[],
        )? {
            changed.push(key.clone());
            record(entity, key, Change::Updated);
        }
    }
    for key in crate::utils::spi::strings(
        &format!(
            "INSERT INTO {qualified_tv} ({columns}) \
             SELECT {columns} FROM {qualified_view} v \
             WHERE NOT EXISTS (SELECT 1 FROM {qualified_tv} t WHERE {same_key}) \
             RETURNING {pk}::text",
            columns = list(&all_columns, "")
        ),
        &[],
    )? {
        changed.push(key.clone());
        record(entity, key, Change::Inserted);
    }
    Ok(changed)
}

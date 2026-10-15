use crate::error::{TViewError, TViewResult};
use pgrx::prelude::*;

/// Drop a TVIEW and all its associated objects
///
/// This function handles the removal of:
/// - The materialized table (`tv_<entity>`)
/// - The backing view (`v_<entity>`)
/// - The metadata record in `pg_tview_meta`
///
/// `tview_name` is `tv_<entity>`, `<entity>` or `schema.tv_<entity>`; a qualified
/// name must name the schema the TVIEW is in. The caller must own it.
///
/// If `if_exists` is true and the TVIEW doesn't exist, a NOTICE is raised instead
/// of an error, like `DROP TABLE IF EXISTS`. Returns whether a TVIEW was dropped.
/// If `cascade` is true, dependent objects are dropped too (mirrors
/// `DROP TABLE … CASCADE`); otherwise the drop is RESTRICT and `PostgreSQL`
/// raises a dependency error when other objects depend on the TVIEW.
/// `PostgreSQL's` transaction system provides automatic atomicity.
///
/// # Errors
/// Returns error if TVIEW doesn't exist (unless `if_exists` is true) or drop operation fails
pub fn drop_tview(tview_name: &str, if_exists: bool, cascade: bool) -> TViewResult<bool> {
    crate::revision::check();
    let crate::catalog::resolve::Name { schema, entity } =
        crate::catalog::resolve::parse(tview_name)?;
    let entity_name = entity.as_str();
    super::lock_entity(entity_name)?;

    // Check if TVIEW exists (in the named schema, if one is named)
    let exists = crate::catalog::row::exists(entity_name)?
        && match &schema {
            Some(schema) => {
                crate::catalog::resolve::registered_schema(entity_name)?.as_ref() == Some(schema)
            }
            None => true,
        };

    if !exists && !if_exists {
        return Err(TViewError::TviewNotFound {
            name: tview_name.to_string(),
        });
    }

    if !exists {
        notice!("TVIEW \"{tview_name}\" does not exist, skipping");
        return Ok(false);
    }

    // Load metadata to get OIDs for schema-safe drops
    // Read leniently: dropping a TVIEW needs its relations, not its plan.
    let meta = crate::catalog::TviewMeta::load_to_rederive(entity_name).map_err(|e| {
        TViewError::SpiError {
            query: "Load TviewMeta by entity".to_string(),
            error: e.to_string(),
        }
    })?;
    if let Some(ref m) = meta
        && let Some(owned) = owned_relation(m)?
    {
        crate::owner::require_owner(owned, &format!("tv_{entity_name}"))?;
    }

    // Remove the TVIEW's triggers from its base tables. They are found by
    // their function and the entity they carry, not through the backing view, which
    // may already be gone when the drop follows a base table or helper view dropped
    // with CASCADE.
    crate::dependency::remove_entity_triggers(entity_name)?;

    // Deregister first: the sql_drop event trigger the drops below fire then finds
    // no TVIEW of its own to clean up, and the drop is recorded once.
    drop_metadata(entity_name)?;

    // Drop the materialized table (schema-resolved via OID).
    // Honor the caller's CASCADE/RESTRICT behavior so an explicit
    // `DROP TABLE tv_* CASCADE` removes dependent objects instead of failing.
    if let Some(ref m) = meta {
        drop_by_oid(m.tview_oid, "TABLE", cascade)?;
        crate::stats::forget(m.tview_oid);
    }

    // Drop the backing view (schema-resolved via OID)
    if let Some(ref m) = meta {
        drop_by_oid(m.view_oid, "VIEW", cascade)?;
    }

    // Invalidate caches since TVIEW was dropped
    crate::cache::invalidate_all();

    // Buffer and flush audit entry immediately (we're in SPI context)
    crate::audit::log_drop(entity_name);
    crate::audit::flush_audit_buffer()?;

    Ok(true)
}

/// Deregister a TVIEW whose backing view or table the current statement dropped
/// as a dependent of something else. Called from the
/// `sql_drop` event trigger only: it first checks that the event dropped the
/// TVIEW's view or table (as a dependent, or by `DROP OWNED`).
///
/// `PostgreSQL` has authorized that drop, and no more. The TVIEW's remaining table
/// is dropped only when the current role owns it; otherwise it is kept as a plain
/// table. The TVIEW's base-table triggers and its registration are removed either
/// way, as their owners.
///
/// # Errors
/// Returns an error outside a `sql_drop` event trigger, or if the event did not
/// drop the TVIEW's view or table.
pub fn handle_dropped(entity: &str) -> TViewResult<()> {
    let args = [crate::utils::spi::text(entity)];
    let dropped = crate::utils::spi::one::<bool>(
        &format!(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger_dropped_objects() d \
             JOIN {} m ON d.objid IN (m.view_oid, m.table_oid) \
             WHERE m.entity = $1 AND d.objsubid = 0 \
               AND d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass)",
            crate::catalog::meta_table()
        ),
        &args,
    )?;
    if dropped != Some(true) {
        return Err(TViewError::InvalidInput {
            parameter: "entity".to_string(),
            reason: format!("the current statement did not drop TVIEW {entity}'s view or table"),
        });
    }

    // Whether the table is left, and whether the current role owns it.
    let (table_left, table_owned) = Spi::get_two_with_args::<bool, bool>(
        &format!(
            "SELECT t.oid IS NOT NULL, \
                    COALESCE(pg_catalog.pg_has_role(t.relowner, 'USAGE'), false) \
             FROM {} m LEFT JOIN pg_catalog.pg_class t ON t.oid = m.table_oid \
             WHERE m.entity = $1",
            crate::catalog::meta_table()
        ),
        &args,
    )
    .map_err(|e| TViewError::SpiError {
        query: "table of a dropped TVIEW".to_string(),
        error: e.to_string(),
    })?;
    if table_owned == Some(true) {
        return drop_tview(entity, true, true).map(|_| ());
    }
    // The table went (with its schema, or its owner's objects): its backing view in
    // the extension's schema goes too, with what depends on it, as it did when it
    // sat in the TVIEW's schema.
    if table_left != Some(true) {
        drop_backing_view(entity)?;
    }

    crate::dependency::remove_entity_triggers(entity)?;
    drop_metadata(entity)?;
    crate::cache::invalidate_all();
    crate::audit::log_drop(entity);
    if table_left == Some(true) {
        notice!(
            "pg_tviews: TVIEW {entity} deregistered; its table belongs to another role and \
             was kept as a plain table"
        );
    }
    Ok(())
}

/// Drop the backing view of `entity` with CASCADE, as the view's owner: the role
/// that dropped the TVIEW's table need not own it.
fn drop_backing_view(entity: &str) -> TViewResult<()> {
    let args = [crate::utils::spi::text(entity)];
    let view = crate::utils::spi::one::<pg_sys::Oid>(
        &format!(
            "SELECT (SELECT v.oid FROM {} m JOIN pg_catalog.pg_class v ON v.oid = m.view_oid \
             WHERE m.entity = $1)",
            crate::catalog::meta_table()
        ),
        &args,
    )?;
    if let Some(view) = view {
        let _owner = crate::owner::AsOwner::of_table(view)?;
        drop_by_oid(view, "VIEW", true)?;
    }
    Ok(())
}

/// The backing views of every registered TVIEW in the extension's schema, read
/// before `DROP EXTENSION pg_tviews` removes the catalog. A view outside
/// it (the layout of releases before 0.1.0-beta.25) is the application's and stays.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn backing_views() -> TViewResult<Vec<pg_sys::Oid>> {
    // An install older than the library (0.1.0 kept its catalog elsewhere, and
    // its backing views were the application's `v_*` views): nothing to drop.
    let current = crate::utils::spi::one::<bool>(
        "SELECT pg_catalog.to_regclass($1) IS NOT NULL",
        &[crate::utils::spi::text(
            crate::catalog::meta_table().as_str(),
        )],
    )?;
    if current != Some(true) {
        return Ok(Vec::new());
    }
    Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT v.oid FROM {} m JOIN pg_catalog.pg_class v ON v.oid = m.view_oid \
                     WHERE v.relnamespace = '{}'::pg_catalog.regnamespace",
                    crate::catalog::meta_table(),
                    crate::utils::ext_schema()
                ),
                None,
                &[],
            )?
            .map(|row| row.get::<pg_sys::Oid>(1))
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|e| TViewError::SpiError {
        query: "backing views of the registered TVIEWs".to_string(),
        error: e.to_string(),
    })
}

/// Drop the backing views the extension left behind once `DROP EXTENSION
/// pg_tviews` has run: they are not extension members, so that `pg_dump` keeps
/// them, and would otherwise outlive it. Their tables stay, as plain
/// tables holding their rows.
///
/// # Errors
/// Returns an error if a view cannot be dropped.
pub fn drop_left_backing_views(views: &[pg_sys::Oid]) -> TViewResult<()> {
    for &view in views {
        let args = [crate::utils::spi::oid(view)];
        let present = crate::utils::spi::one::<bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_class WHERE oid = $1)",
            &args,
        )?;
        if present == Some(true) {
            let _owner = crate::owner::AsOwner::of_table(view)?;
            drop_by_oid(view, "VIEW", true)?;
        }
    }
    Ok(())
}

/// Drop the view at `schema.name`, a TVIEW's backing view name, when it is a
/// leftover: a view no TVIEW is registered with and nothing depends on
/// (a `DROP EXTENSION` in a session that never loaded the library). Returns
/// whether the name is free now.
///
/// # Errors
/// Returns an error if the catalog cannot be read or the view cannot be dropped.
pub fn reclaim_leftover_view(schema: &str, name: &str) -> TViewResult<bool> {
    let args = [
        crate::utils::spi::text(schema),
        crate::utils::spi::text(name),
    ];
    let leftover = crate::utils::spi::one::<pg_sys::Oid>(
        &format!(
            "SELECT (SELECT c.oid FROM pg_catalog.pg_class c \
                     JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind = 'v' \
                       AND NOT EXISTS (SELECT 1 FROM {} m WHERE m.view_oid = c.oid) \
                       AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
                                       WHERE d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
                                         AND d.refobjid = c.oid AND d.deptype = 'n'))",
            crate::catalog::meta_table()
        ),
        &args,
    )?;
    let Some(view) = leftover else {
        return Ok(false);
    };
    {
        let _owner = crate::owner::AsOwner::of_table(view)?;
        drop_by_oid(view, "VIEW", false)?;
    }
    notice!(
        "pg_tviews: dropped {schema}.{name}, a backing view no TVIEW is registered with \
         (left by a dropped extension or TVIEW)"
    );
    Ok(true)
}

/// Resolve a schema-qualified name from an object OID and drop it
///
/// Uses `pg_class JOIN pg_namespace` to find the object's schema at runtime,
/// so drops work regardless of which schema the TVIEW was created in.
///
/// When `cascade` is true the generated statement uses `CASCADE` so dependent
/// objects are removed as well.
fn drop_by_oid(oid: pg_sys::Oid, kind: &str, cascade: bool) -> TViewResult<()> {
    let qualified = crate::utils::spi_get_string(&format!(
        "SELECT quote_ident(n.nspname::text) || '.' || quote_ident(c.relname::text) \
         FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.oid = {}",
        oid.to_u32()
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Resolve qualified name for OID {}", oid.to_u32()),
        pg_error: e.to_string(),
    })?;

    if let Some(qname) = qualified {
        let cascade_kw = if cascade { " CASCADE" } else { "" };
        let sql = format!("DROP {kind} IF EXISTS {qname}{cascade_kw}");
        crate::utils::spi::run_ddl(&sql)?;
    }

    Ok(())
}

/// The relation whose owner may drop the TVIEW: its table, or its view when the
/// table is gone (dropped where the hook did not run). `None` when both are gone:
/// the registration is all that is left, and any role may remove it.
fn owned_relation(meta: &crate::catalog::TviewMeta) -> TViewResult<Option<pg_sys::Oid>> {
    let args = [meta.tview_oid, meta.view_oid].map(crate::utils::spi::oid);
    Spi::connect(|client| {
        client
            .select(
                "SELECT COALESCE((SELECT oid FROM pg_catalog.pg_class WHERE oid = $1), \
                                 (SELECT oid FROM pg_catalog.pg_class WHERE oid = $2))",
                None,
                &args,
            )?
            .first()
            .get_one::<pg_sys::Oid>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Find the relations of TVIEW {}", meta.entity_name),
        pg_error: e.to_string(),
    })
}

/// Drop metadata record from `pg_tview_meta`
fn drop_metadata(entity_name: &str) -> TViewResult<()> {
    let args = [crate::utils::spi::text(entity_name)];
    // With its row in pg_tview_valid: a later table may get its OID.
    let sql = format!(
        "WITH gone AS (DELETE FROM {} WHERE entity = $1 RETURNING table_oid) \
         DELETE FROM {}.pg_tview_valid v USING gone WHERE v.table_oid = gone.table_oid",
        crate::catalog::meta_table(),
        crate::utils::ext_schema()
    );
    // The catalog is written as the extension's owner.
    let _owner = crate::owner::AsOwner::of_extension()?;
    crate::utils::spi::run(&sql, &args)?;

    Ok(())
}

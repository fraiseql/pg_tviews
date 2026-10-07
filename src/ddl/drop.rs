use crate::error::{TViewError, TViewResult};
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Drop a TVIEW and all its associated objects
///
/// This function handles the removal of:
/// - The materialized table (`tv_<entity>`)
/// - The backing view (`v_<entity>`)
/// - The metadata record in `pg_tview_meta`
///
/// `tview_name` is `tv_<entity>`, `<entity>` or `schema.tv_<entity>`; a qualified
/// name must name the schema the TVIEW is in. The caller must own it (issue #134).
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
    let (schema, entity) = super::replace::parse_name(tview_name)?;
    let entity_name = entity.as_str();
    super::lock_entity(entity_name)?;

    // Step 1: Check if TVIEW exists (in the named schema, if one is named)
    let exists = tview_exists_in_metadata(entity_name)?
        && match &schema {
            Some(schema) => {
                super::replace::registered_schema(entity_name)?.as_ref() == Some(schema)
            }
            None => true,
        };

    if !exists && !if_exists {
        return Err(TViewError::MetadataNotFound {
            entity: tview_name.to_string(),
        });
    }

    if !exists {
        notice!("TVIEW \"{tview_name}\" does not exist, skipping");
        return Ok(false);
    }

    // Load metadata to get OIDs for schema-safe drops
    let meta = crate::catalog::TviewMeta::load_by_entity(entity_name).map_err(|e| {
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

    // Step 2: Remove the TVIEW's triggers from its base tables. They are found by
    // their function and the entity they carry, not through the backing view, which
    // may already be gone when the drop follows a base table or helper view dropped
    // with CASCADE (issue #57).
    crate::dependency::remove_entity_triggers(entity_name)?;

    // Step 3: Drop the materialized table (schema-resolved via OID).
    // Honor the caller's CASCADE/RESTRICT behavior so an explicit
    // `DROP TABLE tv_* CASCADE` removes dependent objects instead of failing.
    if let Some(ref m) = meta {
        drop_by_oid(m.tview_oid, "TABLE", cascade)?;
    }

    // Step 4: Drop the backing view (schema-resolved via OID)
    if let Some(ref m) = meta {
        drop_by_oid(m.view_oid, "VIEW", cascade)?;
    }

    // Step 5: Drop metadata record
    drop_metadata(entity_name)?;

    // Invalidate caches since TVIEW was dropped
    crate::queue::cache::invalidate_all_caches();

    // Buffer and flush audit entry immediately (we're in SPI context)
    crate::audit::log_drop(entity_name);
    if let Err(e) = crate::audit::flush_audit_buffer() {
        warning!("Failed to flush audit after DROP: {}", e);
    }

    Ok(true)
}

/// Deregister a TVIEW whose backing view or table the current statement dropped
/// as a dependent of something else (issues #53, #57, #136). Called from the
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
    let args =
        [unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }];
    let dropped = Spi::get_one_with_args::<bool>(
        &format!(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger_dropped_objects() d \
             JOIN {} m ON d.objid IN (m.view_oid, m.table_oid) \
             WHERE m.entity = $1 AND d.objsubid = 0 \
               AND d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass)",
            crate::utils::meta_table()
        ),
        &args,
    )
    .map_err(|e| TViewError::SpiError {
        query: "pg_event_trigger_dropped_objects()".to_string(),
        error: e.to_string(),
    })?;
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
            crate::utils::meta_table()
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
    // sat in the TVIEW's schema (#186).
    if table_left != Some(true) {
        drop_backing_view(entity)?;
    }

    crate::dependency::remove_entity_triggers(entity)?;
    drop_metadata(entity)?;
    crate::queue::cache::invalidate_all_caches();
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
    let args =
        [unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }];
    let view = Spi::get_one_with_args::<pg_sys::Oid>(
        &format!(
            "SELECT (SELECT v.oid FROM {} m JOIN pg_catalog.pg_class v ON v.oid = m.view_oid \
             WHERE m.entity = $1)",
            crate::utils::meta_table()
        ),
        &args,
    )
    .map_err(|e| TViewError::SpiError {
        query: "backing view of a dropped TVIEW".to_string(),
        error: e.to_string(),
    })?;
    if let Some(view) = view {
        let _owner = crate::owner::AsOwner::of_table(view)?;
        drop_by_oid(view, "VIEW", true)?;
    }
    Ok(())
}

/// The backing views of every registered TVIEW, read before `DROP EXTENSION
/// pg_tviews` removes the catalog (#199).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn backing_views() -> TViewResult<Vec<pg_sys::Oid>> {
    Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT v.oid FROM {} m JOIN pg_catalog.pg_class v ON v.oid = m.view_oid",
                    crate::utils::meta_table()
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
/// them, and would otherwise outlive it (#199). Their tables stay, as plain
/// tables holding their rows.
///
/// # Errors
/// Returns an error if a view cannot be dropped.
pub fn drop_left_backing_views(views: &[pg_sys::Oid]) -> TViewResult<()> {
    for &view in views {
        let args =
            [unsafe { DatumWithOid::new(view, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }];
        let present = Spi::get_one_with_args::<bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_class WHERE oid = $1)",
            &args,
        )
        .map_err(|e| TViewError::SpiError {
            query: "backing view left by DROP EXTENSION".to_string(),
            error: e.to_string(),
        })?;
        if present == Some(true) {
            let _owner = crate::owner::AsOwner::of_table(view)?;
            drop_by_oid(view, "VIEW", true)?;
        }
    }
    Ok(())
}

/// Drop the view at `schema.name`, a TVIEW's backing view name, when it is a
/// leftover: a view no TVIEW is registered with and nothing depends on (#199,
/// a `DROP EXTENSION` in a session that never loaded the library). Returns
/// whether the name is free now.
///
/// # Errors
/// Returns an error if the catalog cannot be read or the view cannot be dropped.
pub fn reclaim_leftover_view(schema: &str, name: &str) -> TViewResult<bool> {
    let args = [
        unsafe { DatumWithOid::new(schema, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        unsafe { DatumWithOid::new(name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
    ];
    let leftover = Spi::get_one_with_args::<pg_sys::Oid>(
        &format!(
            "SELECT (SELECT c.oid FROM pg_catalog.pg_class c \
                     JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind = 'v' \
                       AND NOT EXISTS (SELECT 1 FROM {} m WHERE m.view_oid = c.oid) \
                       AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
                                       WHERE d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
                                         AND d.refobjid = c.oid AND d.deptype = 'n'))",
            crate::utils::meta_table()
        ),
        &args,
    )
    .map_err(|e| TViewError::SpiError {
        query: format!("leftover view {schema}.{name}"),
        error: e.to_string(),
    })?;
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
        crate::utils::spi_run_ddl(&sql).map_err(|e| TViewError::SpiError {
            query: sql,
            error: e,
        })?;
    }

    Ok(())
}

/// Check if a TVIEW exists in metadata
fn tview_exists_in_metadata(entity_name: &str) -> TViewResult<bool> {
    let args = vec![unsafe {
        DatumWithOid::new(entity_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
    }];
    Spi::get_one_with_args::<bool>(
        &format!(
            "SELECT COUNT(*) > 0 FROM {} WHERE entity = $1",
            crate::utils::meta_table()
        ),
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check TVIEW metadata: {entity_name}"),
        pg_error: format!("{e:?}"),
    })
    .map(|opt| opt.unwrap_or(false))
}

/// The relation whose owner may drop the TVIEW: its table, or its view when the
/// table is gone (dropped where the hook did not run). `None` when both are gone:
/// the registration is all that is left, and any role may remove it.
fn owned_relation(meta: &crate::catalog::TviewMeta) -> TViewResult<Option<pg_sys::Oid>> {
    let args = [meta.tview_oid, meta.view_oid].map(|oid| {
        // SAFETY: the datum copies the OID.
        unsafe { DatumWithOid::new(oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }
    });
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
    // SAFETY: DatumWithOid::new wraps PostgreSQL datum pointers for SPI parameter passing.
    // The entity name is validated before this call.
    let args =
        [
            unsafe {
                DatumWithOid::new(entity_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
            },
        ];
    let sql = format!(
        "DELETE FROM {} WHERE entity = $1",
        crate::utils::meta_table()
    );
    // The catalog is written as the extension's owner (issue #136).
    let _owner = crate::owner::AsOwner::of_extension()?;
    Spi::run_with_args(&sql, &args).map_err(|e| TViewError::SpiError {
        query: sql.clone(),
        error: e.to_string(),
    })?;

    Ok(())
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_drop_tview_nonexistent_if_exists() {
        // Dropping a non-existent TVIEW with IF EXISTS should not error
        let result = Spi::run("SELECT pg_tviews_drop('nonexistent', true, false)");
        assert!(
            result.is_ok(),
            "IF EXISTS drop of non-existent TVIEW should succeed"
        );
    }

    #[pg_test]
    fn test_drop_tview_nonexistent_strict() {
        // Dropping a non-existent TVIEW without IF EXISTS should error
        let result = Spi::run("SELECT pg_tviews_drop('nonexistent', false, false)");
        assert!(
            result.is_err(),
            "Strict drop of non-existent TVIEW should fail"
        );
    }
}

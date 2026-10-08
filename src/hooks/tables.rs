//! `DROP TABLE tv_*`.

use super::{TViewError, drop_tview, pg_sys, resolve_relation_oid};

/// Handle DROP TABLE tv_*
///
/// Iterates over the parsed `DropStmt.objects` list to correctly handle
/// multi-table statements like `DROP TABLE tv_a, tv_b` and mixed statements
/// like `DROP TABLE regular_table, tv_foo`.
///
/// Returns `Ok(true)` if ALL tables were tv_* and handled, `Ok(false)` if
/// no tv_* tables found (pass-through entirely). For mixed statements
/// containing both tv_* and non-tv_* tables, drops the tv_* ones and returns
/// `Ok(false)` to let the standard handler process the remaining tables.
///
/// Returns `Err` on failures; the caller raises it.
///
/// SAFETY: This function operates on raw `PostgreSQL` C pointers from the `ProcessUtility` hook.
/// All pointers are validated with null checks before dereferencing.
pub(super) unsafe fn handle_drop_table(
    drop_stmt: *mut pg_sys::DropStmt,
    _query_string: *const ::std::os::raw::c_char,
) -> Result<bool, TViewError> {
    // SAFETY: All pointer dereferences are guarded by null checks.
    unsafe {
        if drop_stmt.is_null() {
            return Ok(false);
        }

        let drop_ref = &*drop_stmt;

        // Check if it's dropping a table (not view, index, etc.)
        if drop_ref.removeType != pg_sys::ObjectType::OBJECT_TABLE {
            return Ok(false);
        }

        let objects = drop_ref.objects;
        if objects.is_null() {
            return Ok(false);
        }

        let if_exists = drop_ref.missing_ok;

        // Honor the statement's CASCADE/RESTRICT behavior. Without this the internal
        // drop was always RESTRICT, so `DROP TABLE tv_* CASCADE` on a TVIEW with
        // dependents raised a dependency error that surfaced as an opaque panic.
        let cascade = drop_ref.behavior == pg_sys::DropBehavior::DROP_CASCADE;

        // Collect registered TVIEWs from DropStmt.objects. Each element is a List* of
        // String* name parts ([schema, table] or [table]). A name is claimed only if it
        // resolves (schema-aware, like PostgreSQL) to a relation registered in
        // `pg_tview_meta`; everything else — plain `tv_*` tables, missing names, other
        // tables — is left in the list for the standard handler.
        let num_tables = pg_sys::list_length(objects);
        let mut tv_entries: Vec<(i32, String)> = Vec::new(); // (index, tv_<entity>)
        let mut has_non_tv = false;

        for i in 0..num_tables {
            let name_list = pg_sys::list_nth(objects, i).cast::<pg_sys::List>();
            if name_list.is_null() || pg_sys::list_length(name_list) == 0 {
                has_non_tv = true;
                continue;
            }

            let rv = pg_sys::makeRangeVarFromNameList(name_list);
            let relid = resolve_relation_oid(rv);
            if relid == pg_sys::InvalidOid {
                has_non_tv = true;
                continue;
            }

            match crate::catalog::TviewMeta::load_for_tview(relid) {
                Ok(Some(meta)) => tv_entries.push((i, format!("tv_{}", meta.entity_name))),
                _ => has_non_tv = true,
            }
        }

        if tv_entries.is_empty() {
            return Ok(false);
        }

        // Drop each registered TVIEW via drop_tview
        for (_, name) in &tv_entries {
            drop_tview(name, if_exists, cascade)?;
        }

        // If there were non-tv_* tables, remove tv_* entries from the objects list
        // so the standard handler only processes the remaining non-tv_* tables.
        if has_non_tv {
            // Remove in reverse index order to preserve indices
            for (idx, _) in tv_entries.iter().rev() {
                pg_sys::list_delete_nth_cell(objects, *idx);
            }
            return Ok(false);
        }

        // All tables were tv_* — we handled everything
        Ok(true)
    } // unsafe
}

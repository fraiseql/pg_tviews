//! `DROP TABLE tv_*` and `ALTER TABLE tv_*`.

use super::{CStr, TViewError, drop_tview, pg_sys, resolve_relation_oid};

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

/// Handle ALTER TABLE statements on TVIEW tables
///
/// SAFETY: This function operates on raw `PostgreSQL` C pointers from the `ProcessUtility` hook.
/// All pointers are validated with null checks before dereferencing.
pub(super) unsafe fn handle_alter_table(
    alter_stmt: *mut pg_sys::AlterTableStmt,
    _query_string: *const ::std::os::raw::c_char,
) -> Result<bool, TViewError> {
    // SAFETY: All pointer dereferences are guarded by null checks.
    unsafe {
        if alter_stmt.is_null() {
            return Ok(false);
        }

        let alter_ref = &*alter_stmt;

        // Get the table name
        let relation = alter_ref.relation;
        if relation.is_null() {
            return Ok(false);
        }

        let rel_ref = &*relation;
        let table_name_cstr = rel_ref.relname;
        if table_name_cstr.is_null() {
            return Ok(false);
        }

        let table_name = CStr::from_ptr(table_name_cstr).to_str().unwrap_or("");

        // Check if it's a TVIEW table (starts with tv_)
        if !table_name.starts_with("tv_") {
            return Ok(false);
        }

        // Check the ALTER TABLE commands for SET UNLOGGED/LOGGED
        let cmds = alter_ref.cmds;
        if cmds.is_null() {
            return Ok(false);
        }

        let num_cmds = pg_sys::list_length(cmds);
        for i in 0..num_cmds {
            let cmd_node = pg_sys::list_nth(cmds, i);
            if cmd_node.is_null() {
                continue;
            }

            let cmd = cmd_node.cast::<pg_sys::AlterTableCmd>();
            if cmd.is_null() {
                continue;
            }

            let cmd_ref = &*cmd;

            // Check for SET UNLOGGED or SET LOGGED
            if cmd_ref.subtype == pg_sys::AlterTableType::AT_SetUnLogged {
                // SET UNLOGGED - data is preserved, no special handling needed
                return Ok(false); // Let PostgreSQL handle it normally
            } else if cmd_ref.subtype == pg_sys::AlterTableType::AT_SetLogged {
                // SET LOGGED preserves data — no special handling needed
                return Ok(false); // Let PostgreSQL handle it normally
            }
        }

        // Not a SET UNLOGGED/LOGGED command we care about
        Ok(false)
    }
}

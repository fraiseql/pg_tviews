//! What a utility statement changes that TVIEWs follow, read before it runs.

use super::{CStr, TViewResult, pg_sys};

/// An `ALTER … RENAME COLUMN` statement, captured before it runs.
pub(super) struct ColumnRename {
    pub(super) relation: *const pg_sys::RangeVar,
    pub(super) old_name: String,
    pub(super) new_name: String,
}

impl ColumnRename {
    /// The renamed relation's OID and the old/new names, once the rename has run.
    /// `None` if the relation is gone (`IF EXISTS` on a missing table).
    pub(super) fn resolve(self) -> Option<(pg_sys::Oid, String, String)> {
        // SAFETY: `relation` points into the statement's parse tree, which lives
        // until the utility statement finishes.
        let relid = unsafe {
            pg_sys::RangeVarGetRelidExtended(
                self.relation,
                pg_sys::NoLock.cast_signed(),
                pg_sys::RVROption::RVR_MISSING_OK,
                None,
                std::ptr::null_mut(),
            )
        };
        (relid != pg_sys::InvalidOid).then_some((relid, self.old_name, self.new_name))
    }
}

/// Whether `pg_tviews` is installed in the current database. A syscache lookup:
/// no SPI, and no catalog query that could fail when the extension is absent.
pub(super) fn extension_installed() -> bool {
    // SAFETY: both calls only read backend state; the syscache lookup runs only
    // inside a transaction, where it is valid.
    unsafe {
        pg_sys::IsTransactionState()
            && pg_sys::get_extension_oid(c"pg_tviews".as_ptr(), true) != pg_sys::InvalidOid
    }
}

/// The column rename carried by `pstmt`, if it is one.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt` from the `ProcessUtility` hook.
pub(super) unsafe fn column_rename_of(pstmt: *const pg_sys::PlannedStmt) -> Option<ColumnRename> {
    // SAFETY: every pointer is null-checked before it is dereferenced.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        if (*node).type_ != pg_sys::NodeTag::T_RenameStmt {
            return None;
        }
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → RenameStmt* cast
        let stmt = &*node.cast::<pg_sys::RenameStmt>();
        if stmt.renameType != pg_sys::ObjectType::OBJECT_COLUMN
            || stmt.relation.is_null()
            || stmt.subname.is_null()
            || stmt.newname.is_null()
        {
            return None;
        }
        Some(ColumnRename {
            relation: stmt.relation,
            old_name: CStr::from_ptr(stmt.subname).to_string_lossy().into_owned(),
            new_name: CStr::from_ptr(stmt.newname).to_string_lossy().into_owned(),
        })
    }
}

/// The table an `ALTER TABLE … RENAME TO` or `ALTER TABLE … SET SCHEMA` renames or
/// moves, resolved before the statement runs.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt` from the `ProcessUtility` hook.
pub(super) unsafe fn table_move_of(pstmt: *const pg_sys::PlannedStmt) -> Option<pg_sys::Oid> {
    // SAFETY: every pointer is null-checked before it is dereferenced.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → statement cast by tag
        let relation = match (*node).type_ {
            pg_sys::NodeTag::T_RenameStmt => {
                let stmt = &*node.cast::<pg_sys::RenameStmt>();
                (stmt.renameType == pg_sys::ObjectType::OBJECT_TABLE).then_some(stmt.relation)
            }
            pg_sys::NodeTag::T_AlterObjectSchemaStmt => {
                let stmt = &*node.cast::<pg_sys::AlterObjectSchemaStmt>();
                (stmt.objectType == pg_sys::ObjectType::OBJECT_TABLE).then_some(stmt.relation)
            }
            _ => None,
        }?;
        if relation.is_null() {
            return None;
        }
        let relid = pg_sys::RangeVarGetRelidExtended(
            relation,
            pg_sys::NoLock.cast_signed(),
            pg_sys::RVROption::RVR_MISSING_OK,
            None,
            std::ptr::null_mut(),
        );
        (relid != pg_sys::InvalidOid).then_some(relid)
    }
}

/// The materialized view a `REFRESH MATERIALIZED VIEW` fills (not `WITH NO DATA`,
/// which leaves nothing to read), resolved before the statement runs.
///
/// SAFETY: `pstmt` is null or a valid `PlannedStmt`.
pub(super) unsafe fn matview_refresh_of(pstmt: *const pg_sys::PlannedStmt) -> Option<pg_sys::Oid> {
    // SAFETY: every pointer is null-checked before it is dereferenced.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        if (*node).type_ != pg_sys::NodeTag::T_RefreshMatViewStmt {
            return None;
        }
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → statement cast by tag
        let stmt = &*node.cast::<pg_sys::RefreshMatViewStmt>();
        if stmt.skipData || stmt.relation.is_null() {
            return None;
        }
        let relid = pg_sys::RangeVarGetRelidExtended(
            stmt.relation,
            pg_sys::NoLock.cast_signed(),
            pg_sys::RVROption::RVR_MISSING_OK,
            None,
            std::ptr::null_mut(),
        );
        (relid != pg_sys::InvalidOid).then_some(relid)
    }
}

/// A statement after which the backing views must follow their tables' grants,
/// and their owners when `owners`.
pub(super) struct PrivilegesChange {
    pub(super) owners: bool,
}

/// `GRANT` / `REVOKE` on tables (by name or `ALL TABLES IN SCHEMA`), `ALTER TABLE
/// … OWNER TO` and `REASSIGN OWNED`.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt` from the `ProcessUtility` hook.
pub(super) unsafe fn privileges_change_of(
    pstmt: *const pg_sys::PlannedStmt,
) -> Option<PrivilegesChange> {
    // SAFETY: every pointer is null-checked before it is dereferenced.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → statement cast by tag
        match (*node).type_ {
            pg_sys::NodeTag::T_GrantStmt => {
                let stmt = &*node.cast::<pg_sys::GrantStmt>();
                (stmt.objtype == pg_sys::ObjectType::OBJECT_TABLE)
                    .then_some(PrivilegesChange { owners: false })
            }
            pg_sys::NodeTag::T_AlterTableStmt => {
                let stmt = &*node.cast::<pg_sys::AlterTableStmt>();
                let cmds = stmt.cmds;
                let changes_owner = stmt.objtype == pg_sys::ObjectType::OBJECT_TABLE
                    && (0..pg_sys::list_length(cmds)).any(|i| {
                        let cmd = pg_sys::list_nth(cmds, i).cast::<pg_sys::AlterTableCmd>();
                        !cmd.is_null() && (*cmd).subtype == pg_sys::AlterTableType::AT_ChangeOwner
                    });
                changes_owner.then_some(PrivilegesChange { owners: true })
            }
            pg_sys::NodeTag::T_ReassignOwnedStmt => Some(PrivilegesChange { owners: true }),
            _ => None,
        }
    }
}

/// The tables a `CREATE TABLE … PARTITION OF`, `ALTER TABLE … ATTACH PARTITION` or
/// `… DETACH PARTITION` (also `CONCURRENTLY` and `FINALIZE`) adds to or removes
/// from a partition tree, resolved once `PostgreSQL` has run the statement.
pub(super) struct PartitionDdl {
    pub(super) tables: Vec<*mut pg_sys::RangeVar>,
    /// The partitioned table an `ATTACH` or `DETACH` changes the rows of.
    pub(super) changed_rows_of: Option<*mut pg_sys::RangeVar>,
}

impl PartitionDdl {
    /// Give each added partition the triggers its tree's TVIEWs need, and take
    /// ours off each removed one.
    ///
    /// SAFETY: the `RangeVar`s come from the statement's parse tree, which outlives
    /// the statement.
    pub(super) unsafe fn apply(self) -> TViewResult<()> {
        // Partition roots are cached per backend.
        crate::cache::invalidate_all();
        for rv in self.tables {
            // SAFETY: see above.
            let oid = unsafe { resolve_relation_oid(rv) };
            if oid != pg_sys::InvalidOid {
                crate::dependency::triggers::ensure_partition_triggers(oid)?;
            }
        }
        // The rows of an attached or detached partition enter or leave the
        // partitioned table with no row written: refresh its TVIEWs in full.
        // SAFETY: see above.
        let parent = self
            .changed_rows_of
            .map_or(pg_sys::InvalidOid, |rv| unsafe { resolve_relation_oid(rv) });
        if parent != pg_sys::InvalidOid {
            crate::delta::refresh_tviews_over(parent)?;
        }
        Ok(())
    }
}

/// The partition change carried by `pstmt`, if it is one.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt` from the `ProcessUtility` hook.
pub(super) unsafe fn partition_ddl_of(pstmt: *const pg_sys::PlannedStmt) -> Option<PartitionDdl> {
    // SAFETY: every pointer is null-checked before it is dereferenced.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        let mut tables = Vec::new();
        let mut changed_rows_of = None;
        match (*node).type_ {
            pg_sys::NodeTag::T_CreateStmt => {
                #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → CreateStmt* cast
                let stmt = &*node.cast::<pg_sys::CreateStmt>();
                if !stmt.partbound.is_null() && !stmt.relation.is_null() {
                    tables.push(stmt.relation);
                }
            }
            pg_sys::NodeTag::T_AlterTableStmt => {
                #[allow(clippy::cast_ptr_alignment)]
                // Reason: PostgreSQL Node* → AlterTableStmt* cast
                let stmt = &*node.cast::<pg_sys::AlterTableStmt>();
                for i in 0..pg_sys::list_length(stmt.cmds) {
                    let cmd = pg_sys::list_nth(stmt.cmds, i).cast::<pg_sys::AlterTableCmd>();
                    if cmd.is_null()
                        || !matches!(
                            (*cmd).subtype,
                            pg_sys::AlterTableType::AT_AttachPartition
                                | pg_sys::AlterTableType::AT_DetachPartition
                                | pg_sys::AlterTableType::AT_DetachPartitionFinalize
                        )
                        || (*cmd).def.is_null()
                    {
                        continue;
                    }
                    #[allow(clippy::cast_ptr_alignment)]
                    // Reason: PostgreSQL Node* → PartitionCmd* cast
                    let partition = &*(*cmd).def.cast::<pg_sys::PartitionCmd>();
                    if !partition.name.is_null() {
                        tables.push(partition.name);
                        changed_rows_of = Some(stmt.relation);
                    }
                }
            }
            _ => {}
        }
        (!tables.is_empty()).then_some(PartitionDdl {
            tables,
            changed_rows_of,
        })
    }
}

/// If `node` is `CREATE EXTENSION` or `DROP EXTENSION`, the extension name(s) it names.
///
/// SAFETY: `node` must be a valid, non-null `Node*`.
pub(super) unsafe fn extension_statement_names(node: *mut pg_sys::Node) -> Option<Vec<String>> {
    // SAFETY: the caller's node; each cast follows its tag, and the lists hold
    // the node types PostgreSQL's grammar puts there.
    unsafe {
        let tag = (*node).type_;
        if tag == pg_sys::NodeTag::T_CreateExtensionStmt {
            #[allow(clippy::cast_ptr_alignment)]
            // Reason: PostgreSQL Node* → CreateExtensionStmt* cast
            let stmt = node.cast::<pg_sys::CreateExtensionStmt>();
            let name = if (*stmt).extname.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*stmt).extname)
                    .to_string_lossy()
                    .into_owned()
            };
            return Some(vec![name]);
        }
        if tag == pg_sys::NodeTag::T_DropStmt {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → DropStmt* cast
            let stmt = node.cast::<pg_sys::DropStmt>();
            if (*stmt).removeType != pg_sys::ObjectType::OBJECT_EXTENSION {
                return None;
            }
            let mut names = Vec::new();
            let objects = (*stmt).objects;
            for i in 0..pg_sys::list_length(objects) {
                let item = pg_sys::list_nth(objects, i).cast::<pg_sys::String>();
                if !item.is_null() && !(*item).sval.is_null() {
                    names.push(CStr::from_ptr((*item).sval).to_string_lossy().into_owned());
                }
            }
            return Some(names);
        }
        None
    }
}

/// Whether `pstmt` is a `DROP EXTENSION` naming `pg_tviews`.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt*`.
pub(super) unsafe fn drops_pg_tviews(pstmt: *const pg_sys::PlannedStmt) -> bool {
    // SAFETY: the caller's statement and its utility node, both checked for null.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return false;
        }
        let node = (*pstmt).utilityStmt;
        (*node).type_ == pg_sys::NodeTag::T_DropStmt
            && extension_statement_names(node)
                .is_some_and(|names| names.iter().any(|n| n == "pg_tviews"))
    }
}

/// Resolve a `RangeVar` to a relation OID without locking or raising.
///
/// Returns `InvalidOid` when the relation (or its schema) doesn't exist. This is a
/// direct catalog lookup: no SPI, so it is safe inside the hook's `catch_unwind`.
///
/// SAFETY: `rv` must be null or a valid `RangeVar*`.
pub(super) unsafe fn resolve_relation_oid(rv: *const pg_sys::RangeVar) -> pg_sys::Oid {
    if rv.is_null() {
        return pg_sys::InvalidOid;
    }
    // SAFETY: the caller's RangeVar, checked for null; NoLock with MISSING_OK
    // neither locks nor raises for a missing relation.
    unsafe {
        pg_sys::RangeVarGetRelidExtended(
            rv,
            pg_sys::NoLock.cast_signed(),
            pg_sys::RVROption::RVR_MISSING_OK,
            None,
            std::ptr::null_mut(),
        )
    }
}

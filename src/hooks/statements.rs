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
        let relid = unsafe { resolve_relation_oid(self.relation) };
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

/// An `ALTER TABLE` that renames, drops or retypes columns, or makes the table
/// inherit from another, captured before it runs.
pub(super) struct ColumnChange {
    pub(super) relation: *const pg_sys::RangeVar,
    /// `RENAME COLUMN` or `DROP COLUMN`, when the statement does either.
    pub(super) removes: Option<&'static str>,
    /// Each `ALTER COLUMN … TYPE`: the column and its new type.
    pub(super) retypes: Vec<(String, *const pg_sys::TypeName)>,
    /// Each `INHERIT parent`: the parent.
    pub(super) inherits: Vec<*const pg_sys::RangeVar>,
}

/// The column change carried by the utility statement `node`, if it is one.
///
/// SAFETY: `node` must be a valid, non-null `Node*`.
pub(super) unsafe fn column_change_of(node: *mut pg_sys::Node) -> Option<ColumnChange> {
    // SAFETY: each cast follows the node's tag; the cells of an AlterTableStmt
    // are AlterTableCmds, an AT_AlterColumnType's def a ColumnDef, null-checked.
    unsafe {
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → statement cast by tag
        let change = match (*node).type_ {
            pg_sys::NodeTag::T_RenameStmt => {
                let stmt = &*node.cast::<pg_sys::RenameStmt>();
                if stmt.renameType != pg_sys::ObjectType::OBJECT_COLUMN {
                    return None;
                }
                ColumnChange {
                    relation: stmt.relation,
                    removes: Some("RENAME COLUMN"),
                    retypes: Vec::new(),
                    inherits: Vec::new(),
                }
            }
            pg_sys::NodeTag::T_AlterTableStmt => {
                let stmt = &*node.cast::<pg_sys::AlterTableStmt>();
                let mut change = ColumnChange {
                    relation: stmt.relation,
                    removes: None,
                    retypes: Vec::new(),
                    inherits: Vec::new(),
                };
                for i in 0..pg_sys::list_length(stmt.cmds) {
                    let cmd = pg_sys::list_nth(stmt.cmds, i).cast::<pg_sys::AlterTableCmd>();
                    if cmd.is_null() {
                        continue;
                    }
                    match (*cmd).subtype {
                        pg_sys::AlterTableType::AT_DropColumn => {
                            change.removes = Some("DROP COLUMN");
                        }
                        pg_sys::AlterTableType::AT_AddInherit if !(*cmd).def.is_null() => {
                            change.inherits.push((*cmd).def.cast::<pg_sys::RangeVar>());
                        }
                        pg_sys::AlterTableType::AT_AlterColumnType
                            if !(*cmd).name.is_null() && !(*cmd).def.is_null() =>
                        {
                            let def = (*cmd).def.cast::<pg_sys::ColumnDef>();
                            change.retypes.push((
                                CStr::from_ptr((*cmd).name).to_string_lossy().into_owned(),
                                (*def).typeName,
                            ));
                        }
                        _ => {}
                    }
                }
                change
            }
            // CREATE [FOREIGN] TABLE … INHERITS (…), not a partition. A foreign
            // table's statement starts with its CreateStmt.
            pg_sys::NodeTag::T_CreateStmt | pg_sys::NodeTag::T_CreateForeignTableStmt => {
                let stmt = &*node.cast::<pg_sys::CreateStmt>();
                if !stmt.partbound.is_null() {
                    return None;
                }
                ColumnChange {
                    relation: stmt.relation,
                    removes: None,
                    retypes: Vec::new(),
                    inherits: (0..pg_sys::list_length(stmt.inhRelations))
                        .map(|i| {
                            pg_sys::list_nth(stmt.inhRelations, i)
                                .cast::<pg_sys::RangeVar>()
                                .cast_const()
                        })
                        .filter(|r| !r.is_null())
                        .collect(),
                }
            }
            _ => return None,
        };
        (!change.relation.is_null()
            && (change.removes.is_some()
                || !change.retypes.is_empty()
                || !change.inherits.is_empty()))
        .then_some(change)
    }
}

/// The table an `ALTER TABLE … SET LOGGED` or `SET UNLOGGED` switches, resolved
/// before the statement runs, and whether it becomes LOGGED (the last of them wins,
/// as in PostgreSQL).
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt` from the `ProcessUtility` hook.
pub(super) unsafe fn persistence_change_of(
    pstmt: *const pg_sys::PlannedStmt,
) -> Option<(pg_sys::Oid, bool)> {
    // SAFETY: every pointer is null-checked before it is dereferenced; the cells
    // of an AlterTableStmt are AlterTableCmds.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        if (*node).type_ != pg_sys::NodeTag::T_AlterTableStmt {
            return None;
        }
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → AlterTableStmt* cast
        let stmt = &*node.cast::<pg_sys::AlterTableStmt>();
        let mut logged = None;
        for i in 0..pg_sys::list_length(stmt.cmds) {
            let cmd = pg_sys::list_nth(stmt.cmds, i).cast::<pg_sys::AlterTableCmd>();
            if cmd.is_null() {
                continue;
            }
            match (*cmd).subtype {
                pg_sys::AlterTableType::AT_SetLogged => logged = Some(true),
                pg_sys::AlterTableType::AT_SetUnLogged => logged = Some(false),
                _ => {}
            }
        }
        let logged = logged?;
        if stmt.relation.is_null() {
            return None;
        }
        let relid = resolve_relation_oid(stmt.relation);
        (relid != pg_sys::InvalidOid).then_some((relid, logged))
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
        let relid = resolve_relation_oid(relation);
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
        let relid = resolve_relation_oid(stmt.relation);
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
    /// The partitions an `ATTACH` adds, with their rows: mapped to keys once
    /// attached.
    pub(super) attached: Vec<*mut pg_sys::RangeVar>,
    /// The partitioned table a `DETACH … CONCURRENTLY` (or `FINALIZE`) changes the
    /// rows of across its two transactions: its TVIEWs are refreshed in full. A
    /// plain `DETACH` maps the leaving rows before it runs instead
    /// ([`leaving_partitions_of`]).
    pub(super) changed_rows_of: Option<*mut pg_sys::RangeVar>,
}

impl PartitionDdl {
    /// Give each added partition the triggers its tree's TVIEWs need, and take
    /// ours off each removed one.
    ///
    /// SAFETY: the `RangeVar`s come from the statement's parse tree, which outlives
    /// the statement.
    pub(super) unsafe fn apply(self) -> TViewResult<()> {
        let resolve = |rvs: Vec<*mut pg_sys::RangeVar>| -> Vec<pg_sys::Oid> {
            rvs.into_iter()
                // SAFETY: see above.
                .map(|rv| unsafe { resolve_relation_oid(rv) })
                .filter(|&oid| oid != pg_sys::InvalidOid)
                .collect()
        };
        let tables = resolve(self.tables);
        let attached = resolve(self.attached);
        let parents = resolve(self.changed_rows_of.into_iter().collect());
        // Partition roots are cached per backend.
        crate::cache::invalidate_all();
        for oid in tables {
            crate::dependency::triggers::ensure_partition_triggers(oid)?;
        }
        // The rows of an attached partition enter the tree with no row written:
        // mapped to the keys they reach, as their INSERT would be.
        let mapped = !attached.is_empty();
        for oid in attached {
            crate::delta::map_entering_partition(oid)?;
        }
        if mapped && !crate::executor::inside_writing_statement() {
            crate::flush::flush_refresh_queue()?;
        }
        // The rows a concurrent detach takes out across its two transactions:
        // refresh its TVIEWs in full.
        for parent in parents {
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
        let mut attached = Vec::new();
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
                        match (*cmd).subtype {
                            pg_sys::AlterTableType::AT_AttachPartition => {
                                attached.push(partition.name);
                            }
                            pg_sys::AlterTableType::AT_DetachPartition if !partition.concurrent => {
                            }
                            _ => changed_rows_of = Some(stmt.relation),
                        }
                    }
                }
            }
            _ => {}
        }
        (!tables.is_empty()).then_some(PartitionDdl {
            tables,
            attached,
            changed_rows_of,
        })
    }
}

/// Who may run a statement that takes a partition's rows out of its tree.
enum Access {
    /// The partition's owner, or the given table's (`DETACH`: the parent's).
    Owner(Option<*mut pg_sys::RangeVar>),
    /// A role with `TRUNCATE` on the partition.
    Truncate,
}

/// The partitions whose rows leave their partition tree with the statement, read
/// before it runs: each partition a `DROP TABLE`, `DROP FOREIGN TABLE` or
/// `TRUNCATE` names, and the one a plain `DETACH PARTITION` names. Only when the
/// current user may run the statement (owns the dropped partition or the table it
/// is detached from, may truncate the partition): otherwise it fails before
/// anything changes, and nothing is locked first. With each partition: whether the
/// statement locks its parent too (not `TRUNCATE`).
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt` from the `ProcessUtility` hook.
pub(super) unsafe fn leaving_partitions_of(
    pstmt: *const pg_sys::PlannedStmt,
) -> Vec<(pg_sys::Oid, bool)> {
    // SAFETY: every pointer is null-checked before it is dereferenced; each cast
    // follows the node's tag.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return Vec::new();
        }
        let node = (*pstmt).utilityStmt;
        let owns = |relid: pg_sys::Oid| {
            pg_sys::object_ownercheck(pg_sys::RelationRelationId, relid, pg_sys::GetUserId())
        };
        let may_truncate = |relid: pg_sys::Oid| {
            pg_sys::pg_class_aclcheck(
                relid,
                pg_sys::GetUserId(),
                pg_sys::AclMode::from(pg_sys::ACL_TRUNCATE),
            ) == pg_sys::AclResult::ACLCHECK_OK
        };
        // (partition, who may run the statement: the owner of the partition, or of
        // the table given, or a role with TRUNCATE on the partition)
        let mut names: Vec<(*mut pg_sys::RangeVar, Access)> = Vec::new();
        match (*node).type_ {
            pg_sys::NodeTag::T_TruncateStmt => {
                #[allow(clippy::cast_ptr_alignment)]
                // Reason: PostgreSQL Node* → TruncateStmt* cast
                let stmt = &*node.cast::<pg_sys::TruncateStmt>();
                for i in 0..pg_sys::list_length(stmt.relations) {
                    let rv = pg_sys::list_nth(stmt.relations, i).cast::<pg_sys::RangeVar>();
                    if !rv.is_null() {
                        names.push((rv, Access::Truncate));
                    }
                }
            }
            pg_sys::NodeTag::T_DropStmt => {
                #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → DropStmt* cast
                let stmt = &*node.cast::<pg_sys::DropStmt>();
                if matches!(
                    stmt.removeType,
                    pg_sys::ObjectType::OBJECT_TABLE | pg_sys::ObjectType::OBJECT_FOREIGN_TABLE
                ) {
                    for i in 0..pg_sys::list_length(stmt.objects) {
                        let name_list = pg_sys::list_nth(stmt.objects, i).cast::<pg_sys::List>();
                        if !name_list.is_null() && pg_sys::list_length(name_list) > 0 {
                            names.push((
                                pg_sys::makeRangeVarFromNameList(name_list),
                                Access::Owner(None),
                            ));
                        }
                    }
                }
            }
            pg_sys::NodeTag::T_AlterTableStmt => {
                #[allow(clippy::cast_ptr_alignment)]
                // Reason: PostgreSQL Node* → AlterTableStmt* cast
                let stmt = &*node.cast::<pg_sys::AlterTableStmt>();
                for i in 0..pg_sys::list_length(stmt.cmds) {
                    let cmd = pg_sys::list_nth(stmt.cmds, i).cast::<pg_sys::AlterTableCmd>();
                    if cmd.is_null()
                        || (*cmd).subtype != pg_sys::AlterTableType::AT_DetachPartition
                        || (*cmd).def.is_null()
                    {
                        continue;
                    }
                    #[allow(clippy::cast_ptr_alignment)]
                    // Reason: PostgreSQL Node* → PartitionCmd* cast
                    let partition = &*(*cmd).def.cast::<pg_sys::PartitionCmd>();
                    // DETACH is the detached-from table's owner's to run.
                    if !partition.concurrent && !partition.name.is_null() {
                        names.push((partition.name, Access::Owner(Some(stmt.relation))));
                    }
                }
            }
            _ => {}
        }
        names
            .into_iter()
            .filter_map(|(rv, access)| {
                // A name that does not resolve is PostgreSQL's to report: the
                // privilege checks below raise on InvalidOid.
                let oid = resolve_relation_oid(rv);
                if oid == pg_sys::InvalidOid || !pg_sys::get_rel_relispartition(oid) {
                    return None;
                }
                let allowed = match access {
                    Access::Owner(Some(parent)) => {
                        let parent = resolve_relation_oid(parent);
                        parent != pg_sys::InvalidOid && owns(parent)
                    }
                    Access::Owner(None) => owns(oid),
                    Access::Truncate => may_truncate(oid),
                };
                // TRUNCATE locks the partition alone; DROP and DETACH its parent too.
                allowed.then_some((oid, !matches!(access, Access::Truncate)))
            })
            .collect()
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

/// Resolve a `RangeVar` to a relation OID without locking or raising, under the
/// caller's `search_path` (the hook's own work runs under a fixed one).
///
/// Returns `InvalidOid` when the relation (or its schema) doesn't exist. This is a
/// direct catalog lookup: no SPI, so it is safe inside the hook's `catch_unwind`.
///
/// SAFETY: `rv` must be null or a valid `RangeVar*`.
pub(super) unsafe fn resolve_relation_oid(rv: *const pg_sys::RangeVar) -> pg_sys::Oid {
    if rv.is_null() {
        return pg_sys::InvalidOid;
    }
    let _caller = crate::owner::CallerPath::enter();
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

/// A statement that changes values a TVIEW reads without firing a trigger,
/// resolved before it runs.
pub(super) enum ValueRewrite {
    /// `ALTER TABLE … ALTER COLUMN … SET EXPRESSION` (PostgreSQL 17 and later):
    /// the table and the columns whose generation expression changes.
    #[cfg(not(feature = "pg16"))]
    Expression {
        table: pg_sys::Oid,
        columns: Vec<String>,
    },
    /// `ALTER TYPE … RENAME VALUE`: the enum.
    EnumLabel(pg_sys::Oid),
}

/// The values `pstmt` rewrites without firing a trigger, if it does.
///
/// SAFETY: `pstmt` is null or a valid `PlannedStmt`.
pub(super) unsafe fn value_rewrite_of(pstmt: *const pg_sys::PlannedStmt) -> Option<ValueRewrite> {
    // SAFETY: every pointer is null-checked before it is dereferenced; each cast
    // follows the node's tag.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → statement cast by tag
        match (*node).type_ {
            pg_sys::NodeTag::T_AlterEnumStmt => {
                let stmt = &*node.cast::<pg_sys::AlterEnumStmt>();
                if stmt.oldVal.is_null() || stmt.typeName.is_null() {
                    return None;
                }
                let name = pg_sys::makeTypeNameFromNameList(stmt.typeName);
                let oid = {
                    let _caller = crate::owner::CallerPath::enter();
                    pg_sys::LookupTypeNameOid(std::ptr::null_mut(), name, true)
                };
                (oid != pg_sys::InvalidOid).then_some(ValueRewrite::EnumLabel(oid))
            }
            #[cfg(not(feature = "pg16"))]
            pg_sys::NodeTag::T_AlterTableStmt => {
                let stmt = &*node.cast::<pg_sys::AlterTableStmt>();
                let mut columns = Vec::new();
                for i in 0..pg_sys::list_length(stmt.cmds) {
                    let cmd = pg_sys::list_nth(stmt.cmds, i).cast::<pg_sys::AlterTableCmd>();
                    if !cmd.is_null()
                        && (*cmd).subtype == pg_sys::AlterTableType::AT_SetExpression
                        && !(*cmd).name.is_null()
                    {
                        columns.push(CStr::from_ptr((*cmd).name).to_string_lossy().into_owned());
                    }
                }
                if columns.is_empty() || stmt.relation.is_null() {
                    return None;
                }
                let table = resolve_relation_oid(stmt.relation);
                (table != pg_sys::InvalidOid).then_some(ValueRewrite::Expression { table, columns })
            }
            _ => None,
        }
    }
}

//! What a statement does to the indexes of a TVIEW's table, read before it runs.
//!
//! A user's index may not take a name `pg_tviews` keeps for its own (one it
//! recorded, or one it creates or would create for the definition): its
//! `CREATE INDEX IF NOT EXISTS` would then find the user's index in its place. The
//! statements `pg_tviews_ensure_propagation_indexes()` reports, run by hand, are
//! the exception, and are recorded. A rename or drop of a recorded index is
//! followed.

use super::{CStr, pg_sys};
use crate::catalog::indexes;
use crate::ddl::create::{ManagedIndex, ViewColumns};
use crate::error::{TViewError, TViewResult};

/// What to record once the statement has run.
pub(super) enum IndexFollowUp {
    /// A recorded index renamed.
    Rename {
        table: pg_sys::Oid,
        old: String,
        new: String,
    },
    /// Recorded indexes dropped: each with its table.
    Drop(Vec<(pg_sys::Oid, String)>),
    /// An index `pg_tviews` would create, created by hand.
    Record { table: pg_sys::Oid, name: String },
}

impl IndexFollowUp {
    /// Record what the statement did.
    ///
    /// # Errors
    /// A failed catalog write.
    pub(super) fn run(self) -> TViewResult<()> {
        match self {
            Self::Rename { table, old, new } => indexes::rename(table, &old, &new),
            Self::Drop(dropped) => {
                for (table, name) in dropped {
                    indexes::forget(table, &[name])?;
                }
                Ok(())
            }
            Self::Record { table, name } => indexes::record(table, &[name]),
        }
    }
}

/// A TVIEW's table, as the index statements need it.
struct Tview {
    table: pg_sys::Oid,
    entity: String,
    qualified: String,
}

impl Tview {
    /// The TVIEW whose table is `table`, if it is one.
    fn of(table: pg_sys::Oid) -> TViewResult<Option<Self>> {
        let entity = crate::utils::spi::one::<String>(
            &format!(
                "SELECT entity FROM {} WHERE table_oid = $1::pg_catalog.oid::pg_catalog.regclass",
                crate::utils::meta_table()
            ),
            &[crate::utils::spi::oid(table)],
        )?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        Ok(Some(Self {
            table,
            entity,
            qualified: crate::utils::qualified_relname_from_oid(table)?,
        }))
    }

    /// The names `pg_tviews` keeps on the table: those it recorded, and the
    /// indexes it creates or would create for the definition.
    fn reserved(&self) -> TViewResult<(Vec<String>, Vec<ManagedIndex>)> {
        let recorded = indexes::recorded(self.table)?;
        let meta = crate::catalog::TviewMeta::load_by_entity(&self.entity)?.ok_or_else(|| {
            TViewError::MetadataNotFound {
                entity: self.entity.clone(),
            }
        })?;
        let tview = format!("tv_{}", self.entity);
        let columns = ViewColumns::classify(crate::utils::column_types(meta.view_oid)?);
        let key = &meta.identity.column;
        let lookups: Vec<String> = meta
            .plan
            .lookup_columns()
            .into_iter()
            .filter(|c| *c != key)
            .map(str::to_string)
            .collect();
        let mut candidates = crate::ddl::create::managed_indexes(&tview, &columns, &lookups);
        candidates.extend(
            lookups
                .iter()
                .map(|column| ManagedIndex::propagation(&tview, column, key)),
        );
        Ok((recorded, candidates))
    }

    fn refuse(&self, index: &str) -> TViewError {
        TViewError::IndexNameReserved {
            table: self.qualified.clone(),
            index: index.to_string(),
        }
    }
}

/// Whether a relation named `name` is in the schema of `table`.
fn name_taken(table: pg_sys::Oid, name: &str) -> TViewResult<bool> {
    Ok(crate::utils::spi::one::<bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_class c \
         WHERE c.relname = $2 AND c.relnamespace = \
               (SELECT relnamespace FROM pg_catalog.pg_class WHERE oid = $1))",
        &[crate::utils::spi::oid(table), crate::utils::spi::text(name)],
    )? == Some(true))
}

/// What `pstmt` does to a TVIEW's indexes: refused when it would give a user's
/// index a name `pg_tviews` keeps, else what to record after it runs.
///
/// # Errors
/// [`TViewError::IndexNameReserved`], or a failed catalog read.
///
/// SAFETY: `pstmt` must be null or the hook's valid statement.
pub(super) unsafe fn index_follow_up_of(
    pstmt: *const pg_sys::PlannedStmt,
) -> TViewResult<Option<IndexFollowUp>> {
    // SAFETY: every pointer is null-checked before it is dereferenced; each cast
    // follows the node's tag. `creating_extension` and `IsBinaryUpgrade` are
    // backend state.
    unsafe {
        // An extension script's indexes (pg_tviews' own catalog among them) are no
        // TVIEW's, and the catalog may not exist yet. pg_upgrade restores the
        // indexes and the record as they were.
        if pg_sys::creating_extension
            || pg_sys::IsBinaryUpgrade
            || pstmt.is_null()
            || (*pstmt).utilityStmt.is_null()
        {
            return Ok(None);
        }
        let node = (*pstmt).utilityStmt;
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → statement cast by tag
        match (*node).type_ {
            pg_sys::NodeTag::T_IndexStmt => create_index(&*node.cast::<pg_sys::IndexStmt>()),
            pg_sys::NodeTag::T_RenameStmt => rename_index(&*node.cast::<pg_sys::RenameStmt>()),
            pg_sys::NodeTag::T_DropStmt => drop_index(&*node.cast::<pg_sys::DropStmt>()),
            _ => Ok(None),
        }
    }
}

/// `CREATE INDEX <name> ON <TVIEW table>`.
///
/// SAFETY: `stmt` comes from the hook's statement.
unsafe fn create_index(stmt: &pg_sys::IndexStmt) -> TViewResult<Option<IndexFollowUp>> {
    if stmt.idxname.is_null() {
        return Ok(None);
    }
    // SAFETY: the statement's RangeVar and name, from its parse tree.
    let (table, name) = unsafe {
        (
            super::resolve_relation_oid(stmt.relation),
            CStr::from_ptr(stmt.idxname).to_string_lossy().into_owned(),
        )
    };
    let Some(tview) = Tview::of(table)? else {
        return Ok(None);
    };
    // PostgreSQL refuses the name (or IF NOT EXISTS skips the statement).
    if name_taken(table, &name)? {
        return Ok(None);
    }
    let (recorded, candidates) = tview.reserved()?;
    let candidate = candidates.iter().find(|c| c.name == name);
    if candidate.is_none() && !recorded.contains(&name) {
        return Ok(None);
    }
    if let Some(index) = candidate
        // SAFETY: the hook's statement.
        && unsafe { creates(stmt, index) }
    {
        return Ok(Some(IndexFollowUp::Record { table, name }));
    }
    Err(tview.refuse(&name))
}

/// Whether `stmt` creates exactly the btree index `index`: its columns in order,
/// nothing else (no expression, operator class, collation, ordering, predicate,
/// INCLUDE, uniqueness).
///
/// SAFETY: `stmt` comes from the hook's statement.
unsafe fn creates(stmt: &pg_sys::IndexStmt, index: &ManagedIndex) -> bool {
    // SAFETY: the statement's fields, null-checked; the list cells are IndexElems.
    unsafe {
        let method = if stmt.accessMethod.is_null() {
            "btree".into()
        } else {
            CStr::from_ptr(stmt.accessMethod).to_string_lossy()
        };
        if method != index.method()
            || stmt.unique
            || !stmt.whereClause.is_null()
            || pg_sys::list_length(stmt.indexIncludingParams) != 0
        {
            return false;
        }
        let n = pg_sys::list_length(stmt.indexParams);
        usize::try_from(n).ok() == Some(index.columns.len())
            && (0..n).zip(&index.columns).all(|(i, column)| {
                index_elem_column(pg_sys::list_nth(stmt.indexParams, i).cast())
                    .is_some_and(|c| &c == column)
            })
    }
}

/// `ALTER INDEX … RENAME TO`, or `ALTER TABLE … RENAME TO` naming an index.
///
/// SAFETY: `stmt` comes from the hook's statement.
unsafe fn rename_index(stmt: &pg_sys::RenameStmt) -> TViewResult<Option<IndexFollowUp>> {
    if !matches!(
        stmt.renameType,
        pg_sys::ObjectType::OBJECT_INDEX | pg_sys::ObjectType::OBJECT_TABLE
    ) || stmt.newname.is_null()
    {
        return Ok(None);
    }
    // SAFETY: the statement's RangeVar and new name, from its parse tree.
    let (index, new) = unsafe {
        (
            super::resolve_relation_oid(stmt.relation),
            CStr::from_ptr(stmt.newname).to_string_lossy().into_owned(),
        )
    };
    let Some((table, old)) = relation_of_index(index)? else {
        return Ok(None);
    };
    let Some(tview) = Tview::of(table)? else {
        return Ok(None);
    };
    let (recorded, candidates) = tview.reserved()?;
    if recorded.contains(&old) {
        return Ok(Some(IndexFollowUp::Rename { table, old, new }));
    }
    if recorded.contains(&new) || candidates.iter().any(|c| c.name == new) {
        return Err(tview.refuse(&new));
    }
    Ok(None)
}

/// `DROP INDEX`: the recorded indexes among those it names.
///
/// SAFETY: `stmt` comes from the hook's statement.
unsafe fn drop_index(stmt: &pg_sys::DropStmt) -> TViewResult<Option<IndexFollowUp>> {
    if stmt.removeType != pg_sys::ObjectType::OBJECT_INDEX {
        return Ok(None);
    }
    let mut dropped = Vec::new();
    // SAFETY: a DROP's objects are lists of String nodes naming relations.
    for names in unsafe { string_list(stmt.objects) } {
        // SAFETY: a name list from the parse tree, made into a RangeVar.
        let index = unsafe { super::resolve_relation_oid(pg_sys::makeRangeVarFromNameList(names)) };
        if let Some((table, name)) = relation_of_index(index)?
            && indexes::recorded(table)?.contains(&name)
        {
            dropped.push((table, name));
        }
    }
    Ok((!dropped.is_empty()).then_some(IndexFollowUp::Drop(dropped)))
}

/// The column an `IndexElem` names, when it is a plain column with nothing
/// else (no expression, collation, operator class or ordering).
///
/// SAFETY: `elem` is null or a valid `IndexElem*`.
unsafe fn index_elem_column(elem: *const pg_sys::IndexElem) -> Option<String> {
    // SAFETY: null-checked.
    unsafe {
        let elem = elem.as_ref()?;
        (!elem.name.is_null()
            && elem.expr.is_null()
            && pg_sys::list_length(elem.collation) == 0
            && pg_sys::list_length(elem.opclass) == 0
            && pg_sys::list_length(elem.opclassopts) == 0
            && elem.ordering == pg_sys::SortByDir::SORTBY_DEFAULT
            && elem.nulls_ordering == pg_sys::SortByNulls::SORTBY_NULLS_DEFAULT)
            .then(|| CStr::from_ptr(elem.name).to_string_lossy().into_owned())
    }
}

/// The cells of `list`, each a `List*` (a DROP's object names).
///
/// SAFETY: `list` is null or a valid `List*` of `List*`.
unsafe fn string_list(list: *mut pg_sys::List) -> Vec<*mut pg_sys::List> {
    // SAFETY: indexes within the list's length.
    unsafe {
        (0..pg_sys::list_length(list))
            .map(|i| pg_sys::list_nth(list, i).cast())
            .collect()
    }
}

/// The table and name of index `index`, if it is an index.
fn relation_of_index(index: pg_sys::Oid) -> TViewResult<Option<(pg_sys::Oid, String)>> {
    if index == pg_sys::InvalidOid {
        return Ok(None);
    }
    let row = crate::utils::spi::rows(
        "SELECT i.indrelid, c.relname::pg_catalog.text FROM pg_catalog.pg_index i \
         JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid WHERE i.indexrelid = $1",
        &[crate::utils::spi::oid(index)],
        |row| Ok((row.get::<pg_sys::Oid>(1)?, row.get::<String>(2)?)),
    )?;
    Ok(row
        .into_iter()
        .find_map(|(table, name)| Some((table?, name?))))
}

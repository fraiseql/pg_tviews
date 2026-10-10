//! The read sets of a TVIEW's mapped tables, rendered for its keys: the values
//! a refresh of those keys locks (ADR 0207).

use crate::TViewResult;
use crate::catalog::KeyType;
use crate::lineage::{KeyMapping, Placeholder, ReadSet};
use pgrx::pg_sys::Oid;

/// The query of `set`, a read set of `entity`'s mapping of `mapping`, with the
/// current names of what it reads and `$1` bound as an array of `key_type`
/// (cached until a relation it reads changes). `None` when a relation it reads
/// is gone, or the set locks the whole table.
///
/// # Errors
/// Returns an error if the names cannot be read.
pub fn rendered(
    entity: &str,
    mapping: &KeyMapping,
    set: &ReadSet,
    key_type: &KeyType,
) -> TViewResult<Option<String>> {
    let Some(template) = &set.sql else {
        return Ok(None);
    };
    let key = (entity.to_string(), mapping.relid, set.attnum);
    let sql = if let Some(sql) = crate::cache::READ_SETS.with(|m| m.get(&key)) {
        sql
    } else {
        let sql = crate::lineage::render_template(template)
            .map_err(|e| crate::utils::spi::error(template, &e))?;
        let relids: Vec<Oid> = crate::lineage::template_placeholders(template)
            .into_iter()
            .map(|p| match p {
                Placeholder::Relation(r) | Placeholder::Column(r, _) => Oid::from(r),
            })
            .collect();
        crate::cache::watch(&relids);
        crate::cache::READ_SETS.with(|m| m.insert(key, sql.clone()));
        sql
    };
    let keys = format!("ANY ({})", crate::refresh::key_cast(key_type, "$1", true));
    Ok(sql.map(|sql| sql.replace("ANY ($1)", &keys)))
}

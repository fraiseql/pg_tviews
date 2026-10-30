//! The read sets of a TVIEW's mapped tables, rendered for its keys: the values
//! a refresh of those keys locks (ADR 0207).

use crate::TViewResult;
use crate::lineage::{KeyMapping, Placeholder, ReadSet};
use crate::queue::key::KeyValue;
use pgrx::pg_sys::Oid;

/// The query of `set`, a read set of `entity`'s mapping of `mapping`, with the
/// current names of what it reads and its keys compared with `keys` (cached
/// until a relation it reads changes). `None` when a relation it reads is gone,
/// or the set locks the whole table.
///
/// # Errors
/// Returns an error if the names cannot be read.
pub fn rendered(
    entity: &str,
    mapping: &KeyMapping,
    set: &ReadSet,
    keys: &str,
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
    Ok(sql.map(|sql| sql.replace("ANY ($1)", keys)))
}

/// Refresh side: before `meta`'s rows `keys` are computed, lock (shared) every
/// value they read along their join paths: the read sets of the tables its
/// mappings join, and the keys of the rows of the TVIEWs it embeds (read from
/// its backing view, since a row not stored yet has none). With the intent lock
/// on the TVIEW, which a write refreshing the whole TVIEW conflicts with. Run as
/// the TVIEW's owner, before the rows are locked and computed. The values are
/// locked only once the transaction changed a link
/// ([`super::links_changed`]); the intent lock always.
///
/// # Errors
/// Returns an error if a read-set query fails.
pub fn lock_read_set(meta: &crate::catalog::TviewMeta, keys: &[KeyValue]) -> TViewResult<()> {
    use super::{LockTarget, Side};
    let Some(lookup) = crate::refresh::key_lookup(&meta.key_type, keys)? else {
        return Ok(());
    };
    super::lock_intent(meta.tview_oid.to_u32(), Side::Refresh);
    // A transaction that changed no link computes rows from links every
    // transaction sees committed: a writer of a value they read finds them, and
    // the row locks order the two refreshes. Locking the values too would only
    // make two such writers wait for each other in a cycle (#263).
    if !super::links_changed() {
        return Ok(());
    }
    let entity = meta.entity_name.as_str();
    // Each read set's query, run as one statement: (target, query, its lock
    // value of `v`).
    let mut sets: Vec<(LockTarget, String, String)> = Vec::new();
    for mapping in &meta.plan.tables {
        for set in &mapping.reads {
            let value = if set.attnum == 0 {
                None
            } else {
                super::lock_value(mapping.relid, set.attnum)?
            };
            let (Some(sql), Some(value)) = (rendered(entity, mapping, set, &lookup.rhs)?, value)
            else {
                // No equality to lock by, or a relation or column it reads is gone.
                super::lock_relation(mapping.relid, Side::Refresh);
                continue;
            };
            let target = LockTarget {
                relid: mapping.relid,
                attnums: vec![set.attnum],
            };
            sets.push((target, sql, value.of("v")));
        }
    }
    for embed in &meta.plan.embeds {
        let Some(child) = crate::catalog::TviewMeta::load_by_entity(&embed.entity)? else {
            continue;
        };
        if let Some(sql) = embed_keys_sql(meta, &embed.lookups, &lookup.rhs)? {
            sets.push((
                super::embedded_keys_target(child.tview_oid),
                sql,
                "v".to_string(),
            ));
        }
    }
    if sets.is_empty() {
        return Ok(());
    }
    let sql = sets
        .iter()
        .enumerate()
        .map(|(i, (_, query, value))| format!("SELECT {i}, {value} FROM ({query}) s{i}(v)"))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let mut values: Vec<Vec<String>> = vec![Vec::new(); sets.len()];
    for row in crate::utils::spi::kept_rows(&sql, &[lookup.arg], lookup.plan)? {
        if let [Some(i), Some(value)] = row.as_slice()
            && let Ok(i) = i.parse::<usize>()
            && let Some(set) = values.get_mut(i)
        {
            set.push(value.clone());
        }
    }
    for ((target, _, _), values) in sets.iter().zip(values) {
        super::lock_values(target, Side::Refresh, &values);
    }
    Ok(())
}

/// Refresh side, for a computation of every row of `meta` (a full refresh, a
/// rebuild, a fill): every table its mappings join and every TVIEW it embeds,
/// locked whole.
///
/// # Errors
/// Returns an error if an embedded TVIEW's catalog row cannot be read.
pub fn lock_whole_read_set(meta: &crate::catalog::TviewMeta) -> TViewResult<()> {
    use super::Side;
    for mapping in meta.plan.tables.iter().filter(|m| !m.reads.is_empty()) {
        super::lock_relation(mapping.relid, Side::Refresh);
    }
    for embed in &meta.plan.embeds {
        if let Some(child) = crate::catalog::TviewMeta::load_by_entity(&embed.entity)? {
            super::lock_relation(child.tview_oid.to_u32(), Side::Refresh);
        }
    }
    Ok(())
}

/// `SELECT DISTINCT <lookup>::text FROM <backing view> WHERE <identity> = <keys>`,
/// one per lookup column, combined with `UNION`; `None` without lookups.
fn embed_keys_sql(
    meta: &crate::catalog::TviewMeta,
    lookups: &[String],
    keys: &str,
) -> TViewResult<Option<String>> {
    if lookups.is_empty() {
        return Ok(None);
    }
    let view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
    let identity = crate::utils::ident::quoted(&meta.identity.column);
    Ok(Some(
        lookups
            .iter()
            .map(|lookup| {
                let lookup = crate::utils::ident::quoted(lookup);
                format!(
                    "SELECT DISTINCT {lookup}::pg_catalog.text FROM {view} \
                     WHERE {identity} OPERATOR(pg_catalog.=) {keys} AND {lookup} IS NOT NULL"
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION "),
    ))
}

//! `pg_tviews_flush_and_report()`: the read-model rows a transaction changed, in
//! the GraphQL Cascade shape (issue #76).
//!
//! A mutation function calls it last. It flushes whatever is still queued, then
//! reports every TVIEW row the transaction's refreshes inserted, updated or deleted
//! so far (the journal in [`crate::queue::affected`]), with each row's public `id`
//! and, optionally, its fresh `data`.

use crate::error::{TViewError, TViewResult};
use crate::queue::affected::{self, Change, NetChange};
use crate::utils::quote_identifier;
use pgrx::JsonB;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap};

/// Flush pending refreshes and report the TVIEW rows this transaction changed.
///
/// Returns `{"updated": [...], "deleted": [...], "truncated": bool,
/// "invalidated_types": [...]}`. An updated entry is
/// `{"__typename", "id", "operation": "CREATED" | "UPDATED", "data"}` (`data` only
/// with `include_data`); a deleted entry is `{"__typename", "id"}`. Entries are
/// ordered by entity, then primary key. Past `max_entities` entries, or when the
/// journal overflowed `pg_tviews.report_max_tracked`, `truncated` is true and
/// `invalidated_types` lists the types of the rows left out. With `reset` (the
/// default) the next call reports only what changed after this one.
///
/// # Errors
/// Returns an error if the flush or a catalog/TVIEW read fails.
#[pg_extern]
fn pg_tviews_flush_and_report(
    max_entities: default!(i32, 500),
    include_data: default!(bool, true),
    reset: default!(bool, true),
) -> Result<JsonB, TViewError> {
    crate::queue::flush_refresh_queue()?;
    let (changes, overflow) = affected::summarize(reset);
    let limit = usize::try_from(max_entities).unwrap_or(0);
    Ok(JsonB(build_report(
        &changes,
        &overflow,
        limit,
        include_data,
    )?))
}

/// Set (or with NULL, reset to `PascalCase(entity)`) the GraphQL type name that
/// [`pg_tviews_flush_and_report`] reports for `entity`.
///
/// # Errors
/// Returns an error if the entity is unknown or the name is not a GraphQL name.
#[pg_extern]
fn pg_tviews_set_typename(entity: &str, typename: Option<&str>) -> Result<(), TViewError> {
    if let Some(name) = typename {
        let valid = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(TViewError::InvalidInput {
                parameter: "typename".to_string(),
                reason: format!("'{name}' is not a GraphQL name ([_A-Za-z][_0-9A-Za-z]*)"),
            });
        }
    }
    let updated = Spi::connect_mut(|client| {
        let args = [
            unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            unsafe { DatumWithOid::new(typename, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        ];
        client
            .update(
                &format!(
                    "UPDATE {} SET graphql_typename = $2 WHERE entity = $1",
                    crate::utils::meta_table()
                ),
                None,
                &args,
            )
            .map(|t| t.len())
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Set GraphQL type name".to_string(),
        pg_error: e.to_string(),
    })?;
    if updated == 0 {
        return Err(TViewError::MetadataNotFound {
            entity: entity.to_string(),
        });
    }
    Ok(())
}

/// `blog_post` → `BlogPost`.
fn pascal_case(entity: &str) -> String {
    entity
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect()
}

/// Per-entity type name and qualified TVIEW table.
struct EntityInfo {
    typename: String,
    table: String,
}

fn entity_info(entities: &BTreeSet<&str>) -> TViewResult<HashMap<String, EntityInfo>> {
    let names: Vec<String> = entities.iter().map(|e| (*e).to_string()).collect();
    Spi::connect(|client| {
        let args = [unsafe {
            DatumWithOid::new(names, PgOid::BuiltIn(PgBuiltInOids::TEXTARRAYOID).value())
        }];
        let mut out = HashMap::new();
        for row in client.select(
            &format!(
                "SELECT m.entity, m.graphql_typename, \
                        quote_ident(n.nspname) || '.' || quote_ident(c.relname) AS tbl \
                 FROM {} m \
                 JOIN pg_class c ON c.oid = m.table_oid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE m.entity = ANY($1)",
                crate::utils::meta_table()
            ),
            None,
            &args,
        )? {
            let entity: String = row["entity"].value()?.unwrap_or_default();
            let typename: Option<String> = row["graphql_typename"].value()?;
            let table: String = row["tbl"].value()?.unwrap_or_default();
            out.insert(
                entity.clone(),
                EntityInfo {
                    typename: typename.unwrap_or_else(|| pascal_case(&entity)),
                    table,
                },
            );
        }
        Ok::<_, spi::Error>(out)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Load TVIEWs for the report".to_string(),
        pg_error: e.to_string(),
    })
}

/// Current `id` and `data` of `entity`'s rows `pks`, keyed by pk text.
fn current_rows(
    entity: &str,
    table: &str,
    pks: Vec<String>,
) -> TViewResult<HashMap<String, (Value, Value)>> {
    let qi_pk = quote_identifier(&format!("pk_{entity}"));
    let sql = format!(
        "SELECT t.{qi_pk}::text AS k, to_jsonb(t.*) AS r FROM {table} t \
         WHERE t.{qi_pk}::text = ANY($1)"
    );
    Spi::connect(|client| {
        let args = [unsafe {
            DatumWithOid::new(pks, PgOid::BuiltIn(PgBuiltInOids::TEXTARRAYOID).value())
        }];
        let mut out = HashMap::new();
        for row in client.select(&sql, None, &args)? {
            let key: Option<String> = row["k"].value()?;
            let rec: Option<JsonB> = row["r"].value()?;
            if let (Some(key), Some(JsonB(Value::Object(mut rec)))) = (key, rec) {
                let id = rec.remove("id").unwrap_or(Value::Null);
                let data = rec.remove("data").unwrap_or(Value::Null);
                out.insert(key, (id, data));
            }
        }
        Ok::<_, spi::Error>(out)
    })
    .map_err(|e| TViewError::SpiError {
        query: sql,
        error: e.to_string(),
    })
}

fn build_report(
    changes: &[NetChange],
    overflow: &BTreeSet<String>,
    max_entities: usize,
    include_data: bool,
) -> TViewResult<Value> {
    let entities: BTreeSet<&str> = changes
        .iter()
        .map(|c| c.entity.as_str())
        .chain(overflow.iter().map(String::as_str))
        .collect();
    let info = entity_info(&entities)?;
    let typename = |entity: &str| {
        info.get(entity)
            .map_or_else(|| pascal_case(entity), |i| i.typename.clone())
    };

    // Flush order is not deterministic across runs, so report in a fixed order:
    // entity, then key (numerically when the keys are integers).
    let mut changes = changes.to_vec();
    changes.sort_by(|a, b| {
        a.entity
            .cmp(&b.entity)
            .then_with(|| match (a.pk.parse::<i64>(), b.pk.parse::<i64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                _ => a.pk.cmp(&b.pk),
            })
    });
    let (reported, left_out) = changes.split_at(changes.len().min(max_entities));

    // One read per entity for the rows still present.
    let mut pks_by_entity: HashMap<&str, Vec<String>> = HashMap::new();
    for c in reported {
        if !matches!(c.change, Change::Deleted(_)) {
            pks_by_entity
                .entry(&c.entity)
                .or_default()
                .push(c.pk.clone());
        }
    }
    let mut rows: HashMap<(String, String), (Value, Value)> = HashMap::new();
    for (entity, pks) in pks_by_entity {
        if let Some(i) = info.get(entity) {
            for (pk, row) in current_rows(entity, &i.table, pks)? {
                rows.insert((entity.to_string(), pk), row);
            }
        }
    }

    let mut updated = Vec::new();
    let mut deleted = Vec::new();
    for c in reported {
        let mut item = Map::new();
        item.insert("__typename".into(), Value::String(typename(&c.entity)));
        match &c.change {
            Change::Deleted(id) => {
                item.insert("id".into(), id.clone().map_or(Value::Null, Value::String));
                deleted.push(Value::Object(item));
            }
            change => {
                // A row written then removed by a later, unjournaled path is skipped.
                let Some((id, data)) = rows.remove(&(c.entity.clone(), c.pk.clone())) else {
                    continue;
                };
                let op = if *change == Change::Inserted {
                    "CREATED"
                } else {
                    "UPDATED"
                };
                item.insert("id".into(), id);
                item.insert("operation".into(), Value::String(op.into()));
                if include_data {
                    item.insert("data".into(), data);
                }
                updated.push(Value::Object(item));
            }
        }
    }

    let invalidated: BTreeSet<String> = left_out
        .iter()
        .map(|c| c.entity.as_str())
        .chain(overflow.iter().map(String::as_str))
        .map(typename)
        .collect();
    Ok(json!({
        "updated": updated,
        "deleted": deleted,
        "truncated": !invalidated.is_empty(),
        "invalidated_types": invalidated.into_iter().collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::pascal_case;

    #[test]
    fn pascal_case_joins_snake_parts() {
        assert_eq!(pascal_case("post"), "Post");
        assert_eq!(pascal_case("blog_post"), "BlogPost");
        assert_eq!(pascal_case("order_line_item"), "OrderLineItem");
    }
}

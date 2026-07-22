//! Flush-time direct patch application (issue #56).
//!
//! Consumes the transaction-local patch chains captured by the row trigger and
//! applies them straight to `tv_<entity>` via `jsonb_smart_patch_*` — **zero**
//! backing-view queries. Any pk whose tview row does not yet exist is reported
//! back so the caller recomputes it (a patch can only update an existing row).

use crate::catalog::TviewMeta;
use crate::queue::patch::PatchEntry;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Apply one patch chain to a set of rows of a single entity.
///
/// Generates a grouped `UPDATE tv_<entity> SET data = <nested patch calls> WHERE
/// pk = ANY($n) RETURNING pk`. Patch values are always bound as JSONB parameters
/// — never interpolated. Returns the pks actually updated (from `RETURNING`); the
/// caller diffs these against the input to find rows that must recompute.
pub fn apply_direct_patch(
    meta: &TviewMeta,
    pks: &[i64],
    chain: &[PatchEntry],
) -> spi::Result<Vec<i64>> {
    if pks.is_empty() || chain.is_empty() {
        return Ok(Vec::new());
    }

    let tv_name = crate::utils::relname_from_oid(meta.tview_oid)?;
    let pk_col = format!("pk_{}", meta.entity_name);
    let patch_expr = build_direct_patch_expr(chain);
    let pk_param = chain.len() + 1;

    let sql = format!(
        "UPDATE {tv_name} SET data = {patch_expr}, updated_at = now() \
         WHERE {pk_col} = ANY(${pk_param}) RETURNING {pk_col}"
    );

    // Params: one JSONB per chain entry (in order), then the pk array.
    // SAFETY: DatumWithOid wraps validated structured data (JSONB documents and a
    // BIGINT[]) for SPI parameter passing.
    let json_args: Vec<pgrx::JsonB> = chain
        .iter()
        .map(|(_, fields)| pgrx::JsonB(Value::Object(fields.clone())))
        .collect();
    let pk_vec = pks.to_vec();

    Spi::connect(|client| {
        let mut args: Vec<DatumWithOid> = Vec::with_capacity(chain.len() + 1);
        for j in &json_args {
            args.push(unsafe {
                DatumWithOid::new(
                    pgrx::JsonB(j.0.clone()),
                    PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value(),
                )
            });
        }
        args.push(unsafe {
            DatumWithOid::new(
                pk_vec.clone(),
                PgOid::BuiltIn(PgBuiltInOids::INT8ARRAYOID).value(),
            )
        });

        let rows = client.select(&sql, None, &args)?;
        let mut updated = Vec::new();
        for row in rows {
            if let Some(pk) = row[1].value::<i64>()? {
                updated.push(pk);
            }
        }
        Ok(updated)
    })
}

/// Apply all direct patches for one entity, grouping pks that share an identical
/// chain into a single UPDATE (chunked by `pg_tviews.batch_size`). Increments the
/// applied/fallback counters. Returns the pks that fell back (row not materialised)
/// and must be recomputed by the caller.
pub fn apply_entity_patches(
    meta: &TviewMeta,
    keyed_chains: Vec<(i64, Vec<PatchEntry>)>,
) -> spi::Result<Vec<i64>> {
    // Group pks by identical chain (canonical JSON form).
    let mut groups: HashMap<String, (Vec<PatchEntry>, Vec<i64>)> = HashMap::new();
    for (pk, chain) in keyed_chains {
        let canonical = serde_json::to_string(&chain).unwrap_or_default();
        let entry = groups
            .entry(canonical)
            .or_insert_with(|| (chain, Vec::new()));
        entry.1.push(pk);
    }

    let batch = crate::config::batch_size();
    let mut fallback = Vec::new();
    for (chain, pks) in groups.into_values() {
        for chunk in pks.chunks(batch) {
            let updated = apply_direct_patch(meta, chunk, &chain)?;
            let updated_set: HashSet<i64> = updated.iter().copied().collect();

            crate::metrics::metrics_api::record_direct_patches_applied(updated.len() as u64);
            for &pk in chunk {
                if !updated_set.contains(&pk) {
                    fallback.push(pk);
                }
            }
        }
    }

    if !fallback.is_empty() {
        crate::metrics::metrics_api::record_direct_patch_fallbacks(fallback.len() as u64);
    }
    Ok(fallback)
}

/// Build the nested `jsonb_smart_patch_*` expression for a chain, innermost first
/// (the existing `data` column). Each entry contributes one call and one `$n`
/// JSONB parameter:
/// - empty prefix ⇒ `jsonb_smart_patch_scalar(expr, $n::jsonb)` (top-level merge),
/// - non-empty prefix ⇒ `jsonb_smart_patch_nested(expr, $n::jsonb, ARRAY[...])`.
fn build_direct_patch_expr(chain: &[PatchEntry]) -> String {
    let mut expr = "data".to_string();
    for (i, (prefix, _fields)) in chain.iter().enumerate() {
        let param = i + 1;
        if prefix.is_empty() {
            expr = format!("jsonb_smart_patch_scalar({expr}, ${param}::jsonb)");
        } else {
            let path_literal = prefix
                .iter()
                .map(|seg| format!("'{}'", seg.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ");
            expr =
                format!("jsonb_smart_patch_nested({expr}, ${param}::jsonb, ARRAY[{path_literal}])");
        }
    }
    expr
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn entry(prefix: &[&str], k: &str, v: &str) -> PatchEntry {
        let mut m = Map::new();
        m.insert(k.to_string(), Value::String(v.to_string()));
        (prefix.iter().map(|s| (*s).to_string()).collect(), m)
    }

    #[test]
    fn top_level_chain_builds_scalar_merge() {
        let chain = vec![entry(&[], "bio", "x")];
        assert_eq!(
            build_direct_patch_expr(&chain),
            "jsonb_smart_patch_scalar(data, $1::jsonb)"
        );
    }

    #[test]
    fn nested_prefix_builds_nested_call() {
        let chain = vec![entry(&["author"], "bio", "x")];
        assert_eq!(
            build_direct_patch_expr(&chain),
            "jsonb_smart_patch_nested(data, $1::jsonb, ARRAY['author'])"
        );
    }

    #[test]
    fn two_level_prefix_lists_both_segments() {
        let chain = vec![entry(&["post", "author"], "bio", "x")];
        assert_eq!(
            build_direct_patch_expr(&chain),
            "jsonb_smart_patch_nested(data, $1::jsonb, ARRAY['post', 'author'])"
        );
    }

    #[test]
    fn mixed_chain_nests_calls_in_order() {
        let chain = vec![entry(&[], "title", "t"), entry(&["author"], "bio", "b")];
        assert_eq!(
            build_direct_patch_expr(&chain),
            "jsonb_smart_patch_nested(jsonb_smart_patch_scalar(data, $1::jsonb), $2::jsonb, ARRAY['author'])"
        );
    }

    #[test]
    fn single_quote_in_path_segment_is_escaped() {
        let chain = vec![entry(&["we'ird"], "k", "v")];
        assert_eq!(
            build_direct_patch_expr(&chain),
            "jsonb_smart_patch_nested(data, $1::jsonb, ARRAY['we''ird'])"
        );
    }
}

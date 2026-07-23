//! Flush-time direct patch application (issue #56).
//!
//! Consumes the transaction-local patch chains captured by the row trigger and
//! applies them straight to `tv_<entity>` via `jsonb_smart_patch_*` — **zero**
//! backing-view queries. Any pk whose tview row does not yet exist is reported
//! back so the caller recomputes it (a patch can only update an existing row).

use crate::catalog::{DependencyType, TviewMeta};
use crate::queue::patch::PatchEntry;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Derive a parent's patch chain from a patched child's chain (issue #56).
///
/// When `parent_meta` embeds `child_entity` via a `nested_object` dependency at a
/// concrete path, the child's chain is reproduced at the parent with that path
/// prepended to every entry's prefix — `([], {bio})` for `user` becomes
/// `(["author"], {bio})` for `post`. Multi-level cascades compose by prepending
/// again. Returns `None` (⇒ the parent must recompute) when:
/// - the parent is itself DISTINCT ON or a UNION,
/// - the linkage is UUID-fk based (different plumbing, out of scope),
/// - there is no `fk_<child>` dependency, or it appears more than once (ambiguous),
/// - the dependency is not `nested_object`, or has no/empty path (array/scalar can't
///   take a path patch — interlock with #50).
pub fn derive_parent_chain(
    parent_meta: &TviewMeta,
    child_entity: &str,
    child_chain: &[PatchEntry],
) -> Option<Vec<PatchEntry>> {
    // Parent must itself clear the entity-level gates.
    if !parent_meta.distinct_on_keys.is_empty() || parent_meta.is_union {
        return None;
    }

    let fk_name = format!("fk_{child_entity}");

    // UUID-fk linkage uses different plumbing — decline in this cut.
    if parent_meta.uuid_fk_columns.contains(&fk_name) {
        return None;
    }

    // Locate the dependency by fk column; ambiguous (embedded under multiple keys)
    // or absent ⇒ decline.
    let mut idx = None;
    for (i, col) in parent_meta.fk_columns.iter().enumerate() {
        if *col == fk_name {
            if idx.is_some() {
                return None; // more than one — ambiguous
            }
            idx = Some(i);
        }
    }
    let idx = idx?;

    // Must be a NestedObject dependency with a concrete, non-empty path.
    if parent_meta.dependency_types.get(idx)? != &DependencyType::NestedObject {
        return None;
    }
    let path = parent_meta.dependency_paths.get(idx)?.as_ref()?;
    if path.is_empty() {
        return None;
    }

    // Prepend the dependency path to every chain entry's prefix.
    let derived = child_chain
        .iter()
        .map(|(prefix, fields)| {
            let mut new_prefix = path.clone();
            new_prefix.extend(prefix.iter().cloned());
            (new_prefix, fields.clone())
        })
        .collect();
    Some(derived)
}

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
    let (patch_expr, path_args) = build_direct_patch_expr(chain);
    let pk_param = chain.len() + 1;

    let sql = format!(
        "UPDATE {tv_name} SET data = {patch_expr}, updated_at = now() \
         WHERE {pk_col} = ANY(${pk_param}) RETURNING {pk_col}"
    );

    // Params (all bound, nothing interpolated): one JSONB per chain entry, then the
    // pk array, then one text[] per nested-entry path.
    // SAFETY: DatumWithOid wraps validated structured data (JSONB documents, a
    // BIGINT[], and TEXT[] paths from catalog-parsed dependency metadata) for SPI.
    let json_args: Vec<pgrx::JsonB> = chain
        .iter()
        .map(|(_, fields)| pgrx::JsonB(Value::Object(fields.clone())))
        .collect();
    let pk_vec = pks.to_vec();

    Spi::connect(|client| {
        let mut args: Vec<DatumWithOid> = Vec::with_capacity(chain.len() + 1 + path_args.len());
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
        for path in &path_args {
            args.push(unsafe {
                DatumWithOid::new(
                    path.clone(),
                    PgOid::BuiltIn(PgBuiltInOids::TEXTARRAYOID).value(),
                )
            });
        }

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
/// (the existing `data` column), and collect the path arrays to bind.
///
/// Everything is parameterized — no value or identifier is interpolated. Parameter
/// layout for a chain of `n` entries: `$1..$n` = the JSONB fields (one per entry),
/// `$(n+1)` = the pk array, `$(n+2)..` = one `text[]` per nested entry (in order).
/// Each entry contributes one call:
/// - empty prefix ⇒ `jsonb_smart_patch_scalar(expr, $i::jsonb)` (top-level merge),
/// - non-empty prefix ⇒ `jsonb_smart_patch_nested(expr, $i::jsonb, $p::text[])`
///   with the path bound as a parameter (never an interpolated `ARRAY['…']`).
///
/// Returns `(sql_expr, path_args)` where `path_args` are the path arrays to bind
/// after the JSONB fields and the pk array, in order.
fn build_direct_patch_expr(chain: &[PatchEntry]) -> (String, Vec<Vec<String>>) {
    let n = chain.len();
    let mut expr = "data".to_string();
    let mut path_args: Vec<Vec<String>> = Vec::new();
    for (i, (prefix, _fields)) in chain.iter().enumerate() {
        let json_param = i + 1;
        if prefix.is_empty() {
            expr = format!("jsonb_smart_patch_scalar({expr}, ${json_param}::jsonb)");
        } else {
            // Path params follow the n JSONB params and the single pk-array param.
            let path_param = n + 2 + path_args.len();
            path_args.push(prefix.clone());
            expr = format!(
                "jsonb_smart_patch_nested({expr}, ${json_param}::jsonb, ${path_param}::text[])"
            );
        }
    }
    (expr, path_args)
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
        let (expr, paths) = build_direct_patch_expr(&[entry(&[], "bio", "x")]);
        assert_eq!(expr, "jsonb_smart_patch_scalar(data, $1::jsonb)");
        assert!(paths.is_empty());
    }

    #[test]
    fn nested_prefix_binds_path_param() {
        // Chain of 1 ⇒ $1 = fields, $2 = pk array, $3 = the path text[].
        let (expr, paths) = build_direct_patch_expr(&[entry(&["author"], "bio", "x")]);
        assert_eq!(
            expr,
            "jsonb_smart_patch_nested(data, $1::jsonb, $3::text[])"
        );
        assert_eq!(paths, vec![vec!["author".to_string()]]);
    }

    #[test]
    fn two_level_path_bound_as_array_param() {
        let (expr, paths) = build_direct_patch_expr(&[entry(&["post", "author"], "bio", "x")]);
        assert_eq!(
            expr,
            "jsonb_smart_patch_nested(data, $1::jsonb, $3::text[])"
        );
        assert_eq!(paths, vec![vec!["post".to_string(), "author".to_string()]]);
    }

    #[test]
    fn mixed_chain_nests_calls_and_numbers_path_after_pk() {
        // 2 entries ⇒ $1,$2 = fields, $3 = pk array, $4 = the nested entry's path.
        let chain = vec![entry(&[], "title", "t"), entry(&["author"], "bio", "b")];
        let (expr, paths) = build_direct_patch_expr(&chain);
        assert_eq!(
            expr,
            "jsonb_smart_patch_nested(jsonb_smart_patch_scalar(data, $1::jsonb), $2::jsonb, $4::text[])"
        );
        assert_eq!(paths, vec![vec!["author".to_string()]]);
    }

    #[test]
    fn exotic_path_segment_is_bound_not_interpolated() {
        // A quote in a segment is carried verbatim as a bound parameter — never
        // escaped into SQL — so there is no interpolation surface at all.
        let (expr, paths) = build_direct_patch_expr(&[entry(&["we'ird"], "k", "v")]);
        assert_eq!(
            expr,
            "jsonb_smart_patch_nested(data, $1::jsonb, $3::text[])"
        );
        assert_eq!(paths, vec![vec!["we'ird".to_string()]]);
    }

    // ── derive_parent_chain (issue #56) ─────────────────────────────────────

    /// A parent `TviewMeta` with a single dependency on `fk_<child>`.
    fn parent_meta(fk: &str, dep: DependencyType, path: Option<Vec<String>>) -> TviewMeta {
        TviewMeta {
            fk_columns: vec![fk.to_string()],
            dependency_types: vec![dep],
            dependency_paths: vec![path],
            ..TviewMeta::default()
        }
    }

    #[test]
    fn nested_object_dep_prepends_path() {
        let meta = parent_meta(
            "fk_user",
            DependencyType::NestedObject,
            Some(vec!["author".to_string()]),
        );
        let child = vec![entry(&[], "bio", "x")];
        let derived = derive_parent_chain(&meta, "user", &child).unwrap();
        assert_eq!(derived.len(), 1);
        assert_eq!(derived[0].0, vec!["author".to_string()]);
        assert_eq!(derived[0].1.get("bio").unwrap(), &Value::String("x".into()));
    }

    #[test]
    fn array_dep_declines() {
        let meta = parent_meta(
            "fk_comment",
            DependencyType::Array,
            Some(vec!["comments".to_string()]),
        );
        assert!(derive_parent_chain(&meta, "comment", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn scalar_dep_declines() {
        let meta = parent_meta("fk_user", DependencyType::Scalar, None);
        assert!(derive_parent_chain(&meta, "user", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn nested_without_path_declines() {
        let meta = parent_meta("fk_user", DependencyType::NestedObject, None);
        assert!(derive_parent_chain(&meta, "user", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn uuid_fk_linkage_declines() {
        let mut meta = parent_meta(
            "fk_user",
            DependencyType::NestedObject,
            Some(vec!["author".to_string()]),
        );
        meta.uuid_fk_columns = vec!["fk_user".to_string()];
        assert!(derive_parent_chain(&meta, "user", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn duplicate_embedding_declines() {
        let mut meta = parent_meta(
            "fk_user",
            DependencyType::NestedObject,
            Some(vec!["author".to_string()]),
        );
        // Same fk appears twice → ambiguous which key to patch.
        meta.fk_columns.push("fk_user".to_string());
        meta.dependency_types.push(DependencyType::NestedObject);
        meta.dependency_paths.push(Some(vec!["editor".to_string()]));
        assert!(derive_parent_chain(&meta, "user", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn missing_dependency_declines() {
        let meta = parent_meta(
            "fk_other",
            DependencyType::NestedObject,
            Some(vec!["x".to_string()]),
        );
        assert!(derive_parent_chain(&meta, "user", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn distinct_on_or_union_parent_declines() {
        let mut meta = parent_meta(
            "fk_user",
            DependencyType::NestedObject,
            Some(vec!["author".to_string()]),
        );
        meta.distinct_on_keys = vec!["id".to_string()];
        assert!(derive_parent_chain(&meta, "user", &[entry(&[], "b", "x")]).is_none());

        let mut meta2 = parent_meta(
            "fk_user",
            DependencyType::NestedObject,
            Some(vec!["author".to_string()]),
        );
        meta2.is_union = true;
        assert!(derive_parent_chain(&meta2, "user", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn two_level_composition_prepends_both_prefixes() {
        // Child `user` already embedded at ["author"] within `post`; now `feed`
        // embeds `post` at ["post"]. A user patch derived for post as
        // (["author"], …) composes at feed as (["post","author"], …).
        let feed_meta = parent_meta(
            "fk_post",
            DependencyType::NestedObject,
            Some(vec!["post".to_string()]),
        );
        let post_chain = vec![entry(&["author"], "bio", "x")];
        let derived = derive_parent_chain(&feed_meta, "post", &post_chain).unwrap();
        assert_eq!(derived[0].0, vec!["post".to_string(), "author".to_string()]);
    }
}

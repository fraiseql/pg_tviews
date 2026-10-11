//! Flush-time direct patch application.
//!
//! Consumes the transaction-local patch chains captured by the row trigger and
//! applies them straight to `tv_<entity>` via `jsonb_smart_patch_*` — **zero**
//! backing-view queries. Any pk whose tview row does not yet exist is reported
//! back so the caller recomputes it (a patch can only update an existing row).

use crate::TViewResult;
use crate::catalog::TviewMeta;
use crate::queue::patch::PatchEntry;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Derive a parent's patch chain from a patched child's chain.
///
/// When `parent_meta`'s plan embeds `child_entity`'s document at a concrete path
/// (a nested embed), the child's chain is reproduced at the parent with that path
/// prepended to every entry's prefix — `([], {bio})` for `user` becomes
/// `(["author"], {bio})` for `post`. Multi-level cascades compose by prepending
/// again. Returns `None` (⇒ the parent must recompute) when the parent is itself
/// DISTINCT ON or a set operation, or the embed is not nested at a non-empty path
/// (an array or scalar embed can't take a path patch).
pub fn derive_parent_chain(
    parent_meta: &TviewMeta,
    child_entity: &str,
    child_chain: &[PatchEntry],
) -> Option<Vec<PatchEntry>> {
    if parent_meta.identity.kind == crate::lineage::IdentityKind::DistinctOn
        || parent_meta.plan.set_operation
    {
        return None;
    }
    let embed = parent_meta.plan.embed(child_entity)?;
    if embed.kind != crate::lineage::EmbedKind::Nested || embed.path.is_empty() {
        return None;
    }
    let path = &embed.path;

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
/// Generates a grouped `UPDATE tv_<entity> SET data = <nested patch calls>` over
/// `pk = ANY($n)`, guarded by `data IS DISTINCT FROM <patched>` so a patch that
/// changes nothing writes nothing. Patch values are always bound as
/// JSONB parameters — never interpolated.
///
/// Returns `(pk, changed)` for every **materialised** row among `pks`: the
/// guarded UPDATE runs in a data-modifying CTE and the outer SELECT reads the
/// statement snapshot, so unchanged rows are still reported as present. The
/// caller diffs these against the input to find rows that must recompute (not
/// yet materialised).
pub fn apply_direct_patch(
    meta: &TviewMeta,
    pks: &[i64],
    chain: &[PatchEntry],
) -> TViewResult<Vec<(i64, bool)>> {
    if pks.is_empty() || chain.is_empty() {
        return Ok(Vec::new());
    }

    let qi_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qi_pk = crate::utils::ident::quoted(&format!("pk_{}", meta.entity_name));
    let schema = crate::jsonb_delta::require_jsonb_delta_schema()?;
    let (patch_expr, path_args) = build_direct_patch_expr(&schema, chain);
    let pk_param = chain.len() + 1;

    let sql = format!(
        "WITH changed AS ( \
             UPDATE {qi_tv} SET data = {patch_expr}, updated_at = now() \
             WHERE {qi_pk} = ANY(${pk_param}) AND data IS DISTINCT FROM {patch_expr} \
             RETURNING {qi_pk}) \
         SELECT t.{qi_pk}, t.{qi_pk} IN (SELECT {qi_pk} FROM changed) \
         FROM {qi_tv} t WHERE t.{qi_pk} = ANY(${pk_param})"
    );

    // Params (all bound, nothing interpolated): one JSONB per chain entry, then the
    // pk array, then one text[] per nested-entry path.
    let json_args: Vec<pgrx::JsonB> = chain
        .iter()
        .map(|(_, fields)| pgrx::JsonB(Value::Object(fields.clone())))
        .collect();
    let pk_vec = pks.to_vec();

    Spi::connect(|client| {
        let mut args: Vec<DatumWithOid> = Vec::with_capacity(chain.len() + 1 + path_args.len());
        for j in &json_args {
            args.push(crate::utils::spi::jsonb(pgrx::JsonB(j.0.clone())));
        }
        args.push(crate::utils::spi::int8_array(pk_vec.clone()));
        for path in &path_args {
            args.push(crate::utils::spi::text_array(path.clone()));
        }

        let rows = client.select(&sql, None, &args)?;
        let mut materialised = Vec::new();
        for row in rows {
            if let (Some(pk), Some(changed)) = (row[1].value::<i64>()?, row[2].value::<bool>()?) {
                materialised.push((pk, changed));
            }
        }
        Ok(materialised)
    })
}

/// Write fan-out patches into `tv_<entity>`: for each `(key, fields)`,
/// merge `fields` into the `data` of every row whose `lookup_col` equals `key`,
/// in one statement. Rows already holding those values are left alone. Returns
/// the pks of the rows written, which are journaled and counted as applied.
pub fn apply_fanout_patch(
    meta: &TviewMeta,
    lookup_col: &str,
    rows: &[(i64, Map<String, Value>)],
) -> TViewResult<Vec<i64>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let schema = crate::jsonb_delta::require_jsonb_delta_schema()?;
    let qi_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qi_pk = crate::utils::ident::quoted(&format!("pk_{}", meta.entity_name));
    let qi_lookup = crate::utils::ident::quoted(lookup_col);
    let patch = format!("{schema}.jsonb_smart_patch_scalar(t.data, f.patch)");
    let sql = format!(
        "UPDATE {qi_tv} t SET data = {patch}, updated_at = now() \
         FROM jsonb_each($1) AS f(k, patch) \
         WHERE t.{qi_lookup} = f.k::bigint AND t.data IS DISTINCT FROM {patch} \
         RETURNING t.{qi_pk}::bigint"
    );
    // One JSONB object `{key: fields}` carries every target group.
    let by_key: Map<String, Value> = rows
        .iter()
        .map(|(key, fields)| (key.to_string(), Value::Object(fields.clone())))
        .collect();

    let changed: Vec<i64> = Spi::connect_mut(|client| {
        let args = [crate::utils::spi::jsonb(pgrx::JsonB(Value::Object(by_key)))];
        client
            .update(&sql, None, &args)?
            .map(|row| row[1].value::<i64>())
            .filter_map(Result::transpose)
            .collect::<pgrx::spi::Result<_>>()
    })?;

    // REPEATABLE READ: the patch reached every row the latest snapshot holds
    // with these values (a row it couldn't see would keep the old value).
    if crate::concurrency::crosscheck::enabled() {
        let keys: Vec<i64> = rows.iter().map(|(key, _)| *key).collect();
        let lookup = format!(
            "SELECT t.{qi_pk}::pg_catalog.text FROM {qi_tv} t \
             WHERE t.{qi_lookup} = ANY ($1::pg_catalog.int8[])"
        );
        let args = [crate::utils::spi::int8_array(keys)];
        // Read-write, as the patch: its snapshot sees this flush's own writes.
        let found = crate::utils::spi::kept_rows(&lookup, &args)?;
        crate::concurrency::crosscheck::discovered_by(&lookup, &args, &found)?;
    }
    crate::metrics::metrics_api::record_direct_patches_applied(changed.len() as u64);
    for &pk in &changed {
        crate::queue::affected::record(
            &meta.entity_name,
            pk.to_string(),
            crate::queue::affected::Change::Updated,
        );
    }
    Ok(changed)
}

/// Apply all direct patches for one entity, grouping pks that share an identical
/// chain into a single UPDATE (chunked by `pg_tviews.batch_size`). Increments the
/// applied/fallback counters. Returns the pks that fell back (row not materialised)
/// and must be recomputed by the caller.
pub fn apply_entity_patches(
    meta: &TviewMeta,
    keyed_chains: Vec<(i64, Vec<PatchEntry>)>,
) -> TViewResult<Vec<i64>> {
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
            let materialised = apply_direct_patch(meta, chunk, &chain)?;
            let present: HashSet<i64> = materialised.iter().map(|&(pk, _)| pk).collect();
            let changed = materialised.iter().filter(|&&(_, c)| c).count() as u64;

            crate::metrics::metrics_api::record_direct_patches_applied(changed);
            crate::metrics::metrics_api::record_noop_skipped(materialised.len() as u64 - changed);
            for &(pk, _) in materialised.iter().filter(|&&(_, c)| c) {
                crate::queue::affected::record(
                    &meta.entity_name,
                    pk.to_string(),
                    crate::queue::affected::Change::Updated,
                );
            }
            for &pk in chunk {
                if !present.contains(&pk) {
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
/// (the existing `data` column), and collect the path arrays to bind. `schema` is
/// the quoted `jsonb_delta` schema the calls are qualified with.
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
fn build_direct_patch_expr(schema: &str, chain: &[PatchEntry]) -> (String, Vec<Vec<String>>) {
    let n = chain.len();
    let mut expr = "data".to_string();
    let mut path_args: Vec<Vec<String>> = Vec::new();
    for (i, (prefix, _fields)) in chain.iter().enumerate() {
        let json_param = i + 1;
        if prefix.is_empty() {
            expr = format!("{schema}.jsonb_smart_patch_scalar({expr}, ${json_param}::jsonb)");
        } else {
            // Path params follow the n JSONB params and the single pk-array param.
            let path_param = n + 2 + path_args.len();
            path_args.push(prefix.clone());
            expr = format!(
                "{schema}.jsonb_smart_patch_nested({expr}, ${json_param}::jsonb, ${path_param}::text[])"
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
        let (expr, paths) = build_direct_patch_expr("jd", &[entry(&[], "bio", "x")]);
        assert_eq!(expr, "jd.jsonb_smart_patch_scalar(data, $1::jsonb)");
        assert!(paths.is_empty());
    }

    #[test]
    fn nested_prefix_binds_path_param() {
        // Chain of 1 ⇒ $1 = fields, $2 = pk array, $3 = the path text[].
        let (expr, paths) = build_direct_patch_expr("jd", &[entry(&["author"], "bio", "x")]);
        assert_eq!(
            expr,
            "jd.jsonb_smart_patch_nested(data, $1::jsonb, $3::text[])"
        );
        assert_eq!(paths, vec![vec!["author".to_string()]]);
    }

    #[test]
    fn two_level_path_bound_as_array_param() {
        let (expr, paths) =
            build_direct_patch_expr("jd", &[entry(&["post", "author"], "bio", "x")]);
        assert_eq!(
            expr,
            "jd.jsonb_smart_patch_nested(data, $1::jsonb, $3::text[])"
        );
        assert_eq!(paths, vec![vec!["post".to_string(), "author".to_string()]]);
    }

    #[test]
    fn mixed_chain_nests_calls_and_numbers_path_after_pk() {
        // 2 entries ⇒ $1,$2 = fields, $3 = pk array, $4 = the nested entry's path.
        let chain = vec![entry(&[], "title", "t"), entry(&["author"], "bio", "b")];
        let (expr, paths) = build_direct_patch_expr("jd", &chain);
        assert_eq!(
            expr,
            "jd.jsonb_smart_patch_nested(jd.jsonb_smart_patch_scalar(data, $1::jsonb), $2::jsonb, $4::text[])"
        );
        assert_eq!(paths, vec![vec!["author".to_string()]]);
    }

    #[test]
    fn exotic_path_segment_is_bound_not_interpolated() {
        // A quote in a segment is carried verbatim as a bound parameter — never
        // escaped into SQL — so there is no interpolation surface at all.
        let (expr, paths) = build_direct_patch_expr("jd", &[entry(&["we'ird"], "k", "v")]);
        assert_eq!(
            expr,
            "jd.jsonb_smart_patch_nested(data, $1::jsonb, $3::text[])"
        );
        assert_eq!(paths, vec![vec!["we'ird".to_string()]]);
    }

    // ── derive_parent_chain ─────────────────────────────────────

    use crate::lineage::EmbedKind;

    /// A parent `TviewMeta` whose plan embeds `child` once.
    fn parent_meta(child: &str, kind: EmbedKind, path: &[&str]) -> TviewMeta {
        let mut meta = TviewMeta::default();
        meta.plan.embeds.push(crate::catalog::plan::PlanEmbed {
            entity: child.to_string(),
            lookups: vec![format!("{child}_pk")],
            kind,
            path: path.iter().map(|s| (*s).to_string()).collect(),
        });
        meta
    }

    #[test]
    fn nested_embed_prepends_path() {
        let meta = parent_meta("user", EmbedKind::Nested, &["author"]);
        let child = vec![entry(&[], "bio", "x")];
        let derived = derive_parent_chain(&meta, "user", &child).unwrap();
        assert_eq!(derived.len(), 1);
        assert_eq!(derived[0].0, vec!["author".to_string()]);
        assert_eq!(derived[0].1.get("bio").unwrap(), &Value::String("x".into()));
    }

    #[test]
    fn array_embed_declines() {
        let meta = parent_meta("comment", EmbedKind::Array, &["comments"]);
        assert!(derive_parent_chain(&meta, "comment", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn scalar_embed_declines() {
        let meta = parent_meta("user", EmbedKind::Scalar, &[]);
        assert!(derive_parent_chain(&meta, "user", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn nested_without_path_declines() {
        // Also what a document placed at two paths gets: no single path to patch.
        let meta = parent_meta("user", EmbedKind::Nested, &[]);
        assert!(derive_parent_chain(&meta, "user", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn missing_embed_declines() {
        let meta = parent_meta("other", EmbedKind::Nested, &["x"]);
        assert!(derive_parent_chain(&meta, "user", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn distinct_on_or_set_operation_parent_declines() {
        let mut meta = parent_meta("user", EmbedKind::Nested, &["author"]);
        meta.identity.kind = crate::lineage::IdentityKind::DistinctOn;
        assert!(derive_parent_chain(&meta, "user", &[entry(&[], "b", "x")]).is_none());

        let mut meta2 = parent_meta("user", EmbedKind::Nested, &["author"]);
        meta2.plan.set_operation = true;
        assert!(derive_parent_chain(&meta2, "user", &[entry(&[], "b", "x")]).is_none());
    }

    #[test]
    fn two_level_composition_prepends_both_prefixes() {
        // Child `user` already embedded at ["author"] within `post`; now `feed`
        // embeds `post` at ["post"]. A user patch derived for post as
        // (["author"], …) composes at feed as (["post","author"], …).
        let feed_meta = parent_meta("post", EmbedKind::Nested, &["post"]);
        let post_chain = vec![entry(&["author"], "bio", "x")];
        let derived = derive_parent_chain(&feed_meta, "post", &post_chain).unwrap();
        assert_eq!(derived[0].0, vec!["post".to_string(), "author".to_string()]);
    }
}

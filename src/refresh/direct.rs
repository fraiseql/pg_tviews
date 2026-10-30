//! Flush-time direct patch application.
//!
//! Consumes the transaction-local patch chains captured by the row trigger and
//! applies them straight to `tv_<entity>` via `jsonb_smart_patch_*` — **zero**
//! backing-view queries. Any pk whose tview row does not yet exist is reported
//! back so the caller recomputes it (a patch can only update an existing row).

use crate::TViewResult;
use crate::catalog::TviewMeta;
use crate::queue::patch::PatchEntry;
use crate::utils::spi::LookupKeys;
use pgrx::datum::DatumWithOid;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// What a patch returns of a row it wrote: its public id and reference keys
/// ([`TviewMeta::returned_keys`]).
type Patched = (Option<String>, Vec<Option<String>>);
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

/// Apply patch chains of one shape (the same prefixes and keys, in order) to
/// rows of a single entity: `rows` holds each pk with its chain.
///
/// One `UPDATE tv_<entity> SET data = <nested patch calls>` joins the pks and
/// their values, bound as two arrays (each row's chain values as a JSON array, an
/// element per entry), guarded by `data IS DISTINCT FROM <patched>` so a patch
/// that changes nothing writes nothing. Nothing is interpolated.
///
/// Returns `(pk, changed, id, reference values)` for every **materialised** row
/// among the pks (a patch of `data` changes no reference column): the
/// guarded UPDATE runs in a data-modifying CTE and the outer SELECT reads the
/// statement snapshot, so unchanged rows are still reported as present. The
/// caller diffs these against the input to find rows that must recompute (not
/// yet materialised).
fn apply_direct_patch(
    meta: &TviewMeta,
    rows: &[(i64, Vec<PatchEntry>)],
) -> TViewResult<Vec<(i64, bool, Patched)>> {
    let Some((_, shape)) = rows.first() else {
        return Ok(Vec::new());
    };
    if shape.is_empty() {
        return Ok(Vec::new());
    }

    let qi_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qi_pk = crate::utils::ident::quoted(&format!("pk_{}", meta.entity_name));
    let schema = crate::jsonb_delta::require_jsonb_delta_schema()?;
    let (patch_expr, path_args) = build_direct_patch_expr(&schema, shape);

    let sql = format!(
        "WITH f AS (SELECT * FROM ROWS FROM (pg_catalog.unnest($1::pg_catalog.int8[]), \
                                             pg_catalog.unnest($2::pg_catalog.jsonb[])) AS f(pk, v)), \
         changed AS ( \
             UPDATE {qi_tv} t SET data = {patch_expr} FROM f \
             WHERE t.{qi_pk} = f.pk AND t.data IS DISTINCT FROM {patch_expr} \
             RETURNING t.{qi_pk}) \
         SELECT t.{qi_pk}, t.{qi_pk} IN (SELECT {qi_pk} FROM changed), {} \
         FROM {qi_tv} t WHERE t.{qi_pk} = ANY($1)",
        meta.returned_keys("t")
    );

    let pks: Vec<i64> = rows.iter().map(|(pk, _)| *pk).collect();
    let values: Vec<pgrx::JsonB> = rows
        .iter()
        .map(|(_, chain)| {
            pgrx::JsonB(Value::Array(
                chain
                    .iter()
                    .map(|(_, fields)| Value::Object(fields.clone()))
                    .collect(),
            ))
        })
        .collect();
    let mut args: Vec<DatumWithOid> = vec![
        crate::utils::spi::int8_array(pks),
        DatumWithOid::from(values),
    ];
    for path in path_args {
        args.push(crate::utils::spi::text_array(path));
    }
    Ok(
        crate::utils::spi::kept_rows(&sql, &args, crate::utils::spi::Plan::Generic)?
            .into_iter()
            .filter_map(|row| match &row[..] {
                [Some(pk), Some(changed), id, keys @ ..] => Some((
                    pk.parse().ok()?,
                    changed == "t",
                    (id.clone(), crate::catalog::collections::key_values(keys)),
                )),
                _ => None,
            })
            .collect(),
    )
}

/// Write fan-out patches into `tv_<entity>`: for each `(key, fields)`, write
/// `fields` (keyed by their paths in `data`) into the `data` of every row whose
/// `lookup_col` equals `key`, with one statement per set of paths. Rows already
/// holding those values are left alone. Returns the pks of the rows written,
/// which are journaled and counted as applied.
pub fn apply_fanout_patch(
    meta: &TviewMeta,
    lookup_col: &str,
    rows: &[(i64, Map<String, Value>)],
) -> TViewResult<Vec<i64>> {
    // The rows writing the same paths, each key with its values in path order.
    type Targets = (Vec<i64>, Vec<pgrx::JsonB>);
    let mut shapes: std::collections::BTreeMap<Vec<String>, Targets> =
        std::collections::BTreeMap::new();
    for (key, fields) in rows {
        let mut paths: Vec<&String> = fields.keys().collect();
        paths.sort();
        let values = paths.iter().map(|p| fields[*p].clone()).collect();
        let (keys, all_values) = shapes
            .entry(paths.into_iter().cloned().collect())
            .or_default();
        keys.push(*key);
        all_values.push(pgrx::JsonB(Value::Array(values)));
    }
    let mut changed = Vec::new();
    for (paths, (keys, values)) in shapes {
        changed.extend(write_fanout(meta, lookup_col, &paths, keys, values)?);
    }

    // REPEATABLE READ: the patch reached every row the latest snapshot holds
    // with these values (a row it couldn't see would keep the old value).
    if crate::concurrency::crosscheck::enabled()
        && let Some(keys) = LookupKeys::of(rows.iter().map(|(key, _)| *key).collect())
    {
        let qi_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
        let qi_pk = crate::utils::ident::quoted(&format!("pk_{}", meta.entity_name));
        let qi_lookup = crate::utils::ident::quoted(lookup_col);
        let lookup = format!(
            "SELECT t.{qi_pk}::pg_catalog.text FROM {qi_tv} t WHERE {}",
            keys.predicate(&format!("t.{qi_lookup}"), 1)
        );
        let args = [keys.arg()];
        // Read-write, as the patch: its snapshot sees this flush's own writes.
        let found = crate::utils::spi::kept_rows(&lookup, &args, keys.plan())?;
        crate::concurrency::crosscheck::discovered_by(&lookup, &args, keys.plan(), &found)?;
    }
    crate::metrics::metrics_api::record_direct_patches_applied(
        &meta.entity_name,
        changed.len() as u64,
    );
    Ok(changed
        .into_iter()
        .map(|(pk, (id, references))| {
            crate::queue::affected::record(crate::queue::affected::Recorded::of(
                meta,
                pk,
                id,
                crate::queue::affected::Change::Updated,
                &references,
                None,
            ));
            pk
        })
        .collect())
}

/// The rows of `meta`'s TVIEW whose `lookup_col` is one of `keys`: the children a
/// fan-out of those keys writes, counted through the lookup index with the
/// statement shapes [`write_fanout`] uses.
///
/// # Errors
/// Returns the query's error.
pub fn count_fanout(meta: &TviewMeta, lookup_col: &str, keys: Vec<i64>) -> TViewResult<u64> {
    let Some(lookup) = LookupKeys::of(keys) else {
        return Ok(0);
    };
    let sql = format!(
        "SELECT pg_catalog.count(*) FROM {} t WHERE {}",
        crate::utils::qualified_relname_from_oid(meta.tview_oid)?,
        lookup.predicate(&format!("t.{}", crate::utils::ident::quoted(lookup_col)), 1)
    );
    let rows = crate::utils::spi::kept_rows(&sql, &[lookup.arg()], lookup.plan())?;
    Ok(rows
        .first()
        .and_then(|row| row.first()?.as_deref()?.parse().ok())
        .unwrap_or(0))
}

/// One UPDATE writing `values[i]` (an array, a value per path) at `paths`
/// (fan-out path keys) into the rows whose `lookup_col` is `keys[i]`: `jsonb_set`
/// at each path, the paths bound as parameters. The pks written, with their
/// public ids and reference values.
///
/// The statement is kept, and shaped by the number of keys ([`LookupKeys`]):
/// a generic plan of a join with an array assumes ten keys, each matching
/// `lookup_col`'s average number of rows, and on a TVIEW whose lookup column
/// has few distinct values it scans the whole table for a few rows. One key,
/// a write to one parent, is compared with `=` from a generic plan, whose
/// estimate is the rows of one value; several are joined from the arrays,
/// planned at each execution for their count.
fn write_fanout(
    meta: &TviewMeta,
    lookup_col: &str,
    paths: &[String],
    keys: Vec<i64>,
    values: Vec<pgrx::JsonB>,
) -> TViewResult<Vec<(i64, Patched)>> {
    let Some(lookup) = LookupKeys::of(keys) else {
        return Ok(Vec::new());
    };
    let qi_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qi_pk = crate::utils::ident::quoted(&format!("pk_{}", meta.entity_name));
    let qi_lookup = crate::utils::ident::quoted(lookup_col);
    // The rows written, and where each one's values come from.
    let (from, filter, value) = match lookup {
        LookupKeys::One(_) => (
            "",
            lookup.predicate(&format!("t.{qi_lookup}"), 1),
            "($2::pg_catalog.jsonb[])[1]",
        ),
        LookupKeys::Many(_) => (
            " FROM ROWS FROM (pg_catalog.unnest($1::pg_catalog.int8[]), \
                              pg_catalog.unnest($2::pg_catalog.jsonb[])) AS f(k, v)",
            format!("t.{qi_lookup} = f.k"),
            "f.v",
        ),
    };
    let patched = (0..paths.len()).fold("t.data".to_string(), |data, i| {
        format!(
            "pg_catalog.jsonb_set({data}, ${}::pg_catalog.text[], {value} -> {i})",
            i + 3
        )
    });
    let sql = format!(
        "UPDATE {qi_tv} t SET data = {patched}{from} \
         WHERE {filter} AND t.data IS DISTINCT FROM {patched} \
         RETURNING t.{qi_pk}::pg_catalog.int8, {}",
        meta.returned_keys("t")
    );
    let mut args = vec![lookup.arg(), DatumWithOid::from(values)];
    for key in paths {
        let path = crate::queue::patch::fanout_path(key).ok_or_else(|| {
            crate::error::TViewError::SerializationError {
                message: format!("fan-out path {key} of tv_{}", meta.entity_name),
            }
        })?;
        args.push(crate::utils::spi::text_array(path));
    }
    Ok(crate::utils::spi::kept_rows(&sql, &args, lookup.plan())?
        .into_iter()
        .filter_map(|mut row| {
            let id = row.get_mut(1).and_then(Option::take);
            let keys = crate::catalog::collections::key_values(row.get(2..).unwrap_or_default());
            Some((row.first()?.as_deref()?.parse().ok()?, (id, keys)))
        })
        .collect())
}

/// Apply all direct patches for one entity, one UPDATE per chain shape (chunked
/// by `pg_tviews.batch_size`). Increments the
/// applied/fallback counters. Returns the pks that fell back (row not materialised)
/// and must be recomputed by the caller.
pub fn apply_entity_patches(
    meta: &TviewMeta,
    keyed_chains: Vec<(i64, Vec<PatchEntry>)>,
) -> TViewResult<Vec<i64>> {
    // Group the rows by the shape of their chains: each entry's prefix and keys.
    // (prefix, keys) of each entry.
    type Shape = Vec<(Vec<String>, Vec<String>)>;
    let mut groups: HashMap<Shape, Vec<(i64, Vec<PatchEntry>)>> = HashMap::new();
    for (pk, chain) in keyed_chains {
        let shape = chain
            .iter()
            .map(|(prefix, fields)| (prefix.clone(), fields.keys().cloned().collect()))
            .collect();
        groups.entry(shape).or_default().push((pk, chain));
    }

    let batch = crate::config::batch_size();
    let mut fallback = Vec::new();
    for rows in groups.into_values() {
        for chunk in rows.chunks(batch) {
            let materialised = apply_direct_patch(meta, chunk)?;
            let present: HashSet<i64> = materialised.iter().map(|&(pk, ..)| pk).collect();
            let changed = materialised.iter().filter(|&&(_, c, _)| c).count() as u64;

            crate::metrics::metrics_api::record_direct_patches_applied(&meta.entity_name, changed);
            crate::metrics::metrics_api::record_noop_skipped(
                &meta.entity_name,
                materialised.len() as u64 - changed,
            );
            for (pk, _, (id, references)) in materialised.into_iter().filter(|&(_, c, _)| c) {
                crate::queue::affected::record(crate::queue::affected::Recorded::of(
                    meta,
                    pk,
                    id,
                    crate::queue::affected::Change::Updated,
                    &references,
                    None,
                ));
            }
            for (pk, _) in chunk {
                if !present.contains(pk) {
                    fallback.push(*pk);
                }
            }
        }
    }

    if !fallback.is_empty() {
        crate::metrics::metrics_api::record_direct_patch_fallbacks(
            &meta.entity_name,
            fallback.len() as u64,
        );
    }
    Ok(fallback)
}

/// Build the nested `jsonb_smart_patch_*` expression for a chain shape,
/// innermost first (the existing `t.data`), and collect the path arrays to bind.
/// `schema` is the quoted `jsonb_delta` schema the calls are qualified with.
///
/// Entry `i`'s fields are `f.v -> i` (the row's values, see
/// [`apply_direct_patch`]); `$1` and `$2` are the pk and value arrays, and each
/// nested entry's path is bound as `$3..` in order:
/// - empty prefix ⇒ `jsonb_smart_patch_scalar(expr, f.v -> i)` (top-level merge),
/// - non-empty prefix ⇒ `jsonb_smart_patch_nested(expr, f.v -> i, $p::text[])`.
fn build_direct_patch_expr(schema: &str, chain: &[PatchEntry]) -> (String, Vec<Vec<String>>) {
    let mut expr = "t.data".to_string();
    let mut path_args: Vec<Vec<String>> = Vec::new();
    for (i, (prefix, _fields)) in chain.iter().enumerate() {
        if prefix.is_empty() {
            expr = format!("{schema}.jsonb_smart_patch_scalar({expr}, f.v -> {i})");
        } else {
            let path_param = 3 + path_args.len();
            path_args.push(prefix.clone());
            expr = format!(
                "{schema}.jsonb_smart_patch_nested({expr}, f.v -> {i}, ${path_param}::pg_catalog.text[])"
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
        assert_eq!(expr, "jd.jsonb_smart_patch_scalar(t.data, f.v -> 0)");
        assert!(paths.is_empty());
    }

    #[test]
    fn nested_prefix_binds_path_param() {
        // $1 = pk array, $2 = value array, $3 = the path text[].
        let (expr, paths) = build_direct_patch_expr("jd", &[entry(&["author"], "bio", "x")]);
        assert_eq!(
            expr,
            "jd.jsonb_smart_patch_nested(t.data, f.v -> 0, $3::pg_catalog.text[])"
        );
        assert_eq!(paths, vec![vec!["author".to_string()]]);
    }

    #[test]
    fn two_level_path_bound_as_array_param() {
        let (expr, paths) =
            build_direct_patch_expr("jd", &[entry(&["post", "author"], "bio", "x")]);
        assert_eq!(
            expr,
            "jd.jsonb_smart_patch_nested(t.data, f.v -> 0, $3::pg_catalog.text[])"
        );
        assert_eq!(paths, vec![vec!["post".to_string(), "author".to_string()]]);
    }

    #[test]
    fn mixed_chain_nests_calls_and_numbers_paths_after_the_arrays() {
        // 2 entries ⇒ values f.v -> 0 and f.v -> 1, $3 = the nested entry's path.
        let chain = vec![entry(&[], "title", "t"), entry(&["author"], "bio", "b")];
        let (expr, paths) = build_direct_patch_expr("jd", &chain);
        assert_eq!(
            expr,
            "jd.jsonb_smart_patch_nested(jd.jsonb_smart_patch_scalar(t.data, f.v -> 0), f.v -> 1, $3::pg_catalog.text[])"
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
            "jd.jsonb_smart_patch_nested(t.data, f.v -> 0, $3::pg_catalog.text[])"
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

//! Direct-patch column→key extraction (issue #56).
//!
//! Extracts, from a TVIEW's SELECT statement, the set of base-table columns that
//! map *identity-style* to top-level keys of the entity's own `data` JSONB object
//! (e.g. `jsonb_build_object('bio', bio)` ⇒ `bio → bio`). These mappings let the
//! row trigger build a JSONB patch straight from `NEW` without re-querying the
//! backing view (the "direct-patch fast path").
//!
//! The extraction is deliberately **conservative**: anything it is unsure about is
//! omitted. An omitted (or smaller) map only means fewer fast-path hits — never
//! incorrect data, because the recompute path remains the single source of truth.

use crate::schema::parser::{parse_select_columns_with_expressions, split_by_top_level_comma};
use std::collections::HashMap;

/// SQL keywords that can immediately follow the base table in a FROM clause,
/// signalling that the table has no alias (`FROM tb_post WHERE …`).
const FROM_KEYWORDS: &[&str] = &[
    "where", "join", "left", "right", "inner", "outer", "full", "cross", "group", "order", "on",
    "union", "limit", "offset", "having", "using", "natural", "as", "and", "or",
];

/// Extract identity-style `(base_column, jsonb_key)` pairs from the top-level
/// `jsonb_build_object(...)` that produces the entity's `data` output column.
///
/// `base_table` is the entity's base table name (e.g. `"tb_post"`); its FROM-clause
/// alias is resolved internally so `p.title` is accepted when the view says
/// `FROM tb_post p`.
///
/// Only pairs whose value is a *bare* base-table column survive:
/// - unqualified `col`,
/// - `tb_<entity>.col` (base table referenced by name), or
/// - `<alias>.col` (base table's FROM alias).
///
/// Everything else is rejected — omission is the fallback signal:
/// - any expression (`first || ' ' || last`, casts, function calls, `CASE`, …),
/// - columns qualified with another relation (`u.name`, `v_user.data`),
/// - nested `jsonb_build_object` / `jsonb_agg` values (dependency machinery owns those),
/// - duplicate keys or duplicate columns (ambiguous ⇒ recompute).
#[must_use]
pub fn extract_direct_column_map(select_sql: &str, base_table: &str) -> Vec<(String, String)> {
    // 1. Isolate the `data` output column's full expression.
    let Ok(columns) = parse_select_columns_with_expressions(select_sql) else {
        return Vec::new();
    };
    let Some((_, data_expr)) = columns
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("data"))
    else {
        return Vec::new();
    };

    // 2. The expression must be a bare top-level jsonb_build_object(...) call.
    let Some(args_str) = jsonb_build_object_args(data_expr) else {
        return Vec::new();
    };

    // 3. Split the builder arguments into key/value pairs.
    let parts = split_by_top_level_comma(args_str);
    if parts.is_empty() || !parts.len().is_multiple_of(2) {
        return Vec::new();
    }

    // 4. Resolve the base table's FROM-clause alias (if any).
    let alias = resolve_base_alias(select_sql, base_table);

    // 5. Classify each key/value pair.
    let mut pairs: Vec<(String, String)> = Vec::new();
    for pair in parts.chunks_exact(2) {
        let (Some(key), Some(col)) = (
            parse_key_literal(&pair[0]),
            classify_direct_value(&pair[1], base_table, alias.as_deref()),
        ) else {
            continue;
        };
        pairs.push((col, key));
    }

    // 6. Drop ambiguous keys/columns (each must appear exactly once).
    drop_ambiguous(&pairs)
}

/// If `expr` (leading-trimmed) begins with a `jsonb_build_object(` call, return the
/// raw substring between its outermost matching parentheses. `None` otherwise —
/// e.g. the data column is a subquery, a `COALESCE(...)` wrapper, or otherwise not
/// a bare builder call (all of which fall back to recompute).
fn jsonb_build_object_args(expr: &str) -> Option<&str> {
    const FN_NAME: &str = "jsonb_build_object";
    let trimmed = expr.trim_start();

    let prefix = trimmed.get(..FN_NAME.len())?;
    if !prefix.eq_ignore_ascii_case(FN_NAME) {
        return None;
    }
    let after = trimmed[FN_NAME.len()..].trim_start();
    if !after.starts_with('(') {
        return None;
    }

    // Byte offset of the opening '(' within `trimmed` (ASCII: boundary-safe).
    let open_idx = trimmed.len() - after.len();
    let bytes = trimmed.as_bytes();
    let mut depth: i32 = 0;
    let mut in_squote = false;
    let mut in_dquote = false;
    let mut prev = 0u8;
    let mut i = open_idx;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_dquote && prev != b'\\' => in_squote = !in_squote,
            b'"' if !in_squote && prev != b'\\' => in_dquote = !in_dquote,
            b'(' if !in_squote && !in_dquote => depth += 1,
            b')' if !in_squote && !in_dquote => {
                depth -= 1;
                if depth == 0 {
                    return Some(trimmed[open_idx + 1..i].trim());
                }
            }
            _ => {}
        }
        prev = bytes[i];
        i += 1;
    }
    None
}

/// Parse a single-quoted SQL string literal `'key'` into its unquoted text.
///
/// Returns `None` unless `s` is a plain string literal whose contents are a
/// simple identifier-like token (letters, digits, underscore). Rejecting exotic
/// keys keeps later `ARRAY['…']` path building free of quoting/escaping hazards
/// (issue #56 security note) — such keys simply fall back to recompute.
fn parse_key_literal(s: &str) -> Option<String> {
    let inner = s.trim().strip_prefix('\'')?.strip_suffix('\'')?;
    if inner.is_empty() || !is_simple_ident_chars(inner) {
        return None;
    }
    Some(inner.to_string())
}

/// Classify a `jsonb_build_object` value expression. Returns `Some(base_column)`
/// when the value is a bare base-table column reference; `None` otherwise.
fn classify_direct_value(value: &str, base_table: &str, alias: Option<&str>) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }

    // Accept at most one qualifier segment: `col` or `qualifier.col`.
    let mut segs = v.split('.');
    let first = segs.next()?;
    let (qualifier, col) = match segs.next() {
        None => (None, first),
        Some(second) => {
            if segs.next().is_some() {
                return None; // schema.table.col or deeper — reject
            }
            (Some(first), second)
        }
    };

    if !is_simple_ident(col) {
        return None;
    }
    if let Some(q) = qualifier {
        if !is_simple_ident(q) {
            return None;
        }
        let matches_base =
            q.eq_ignore_ascii_case(base_table) || alias.is_some_and(|a| q.eq_ignore_ascii_case(a));
        if !matches_base {
            return None;
        }
    }
    Some(col.to_string())
}

/// Resolve the FROM-clause alias of `base_table`, if the view assigns one:
/// `FROM tb_post p` ⇒ `Some("p")`, `FROM tb_post AS p` ⇒ `Some("p")`,
/// `FROM tb_post` ⇒ `None`.
///
/// Conservative: only the primary FROM table's alias is resolved, so a joined
/// relation's alias is never mistaken for the base table's — accepting a joined
/// alias would let a joined column masquerade as a base column and break
/// byte-identity with the recompute path.
fn resolve_base_alias(select_sql: &str, base_table: &str) -> Option<String> {
    let lower = select_sql.to_lowercase();
    let base_lower = base_table.to_lowercase();

    // Find the first FROM (word-bounded) and the base table immediately after it.
    let from_pos = find_word(&lower, "from", 0)?;
    let rel_pos = find_word(&lower, &base_lower, from_pos + "from".len())?;
    let after_rel = rel_pos + base_lower.len();

    // Work on the original-case tail so the alias keeps its case.
    let tail = select_sql.get(after_rel..)?.trim_start();

    // Skip an optional AS keyword.
    let tail = match tail.get(..2) {
        Some(kw)
            if kw.eq_ignore_ascii_case("as")
                && tail[2..].starts_with(|c: char| c.is_whitespace()) =>
        {
            tail[2..].trim_start()
        }
        _ => tail,
    };

    // The alias is the leading identifier token.
    let alias: String = tail
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if alias.is_empty() {
        return None;
    }

    // A SQL keyword here means the base table had no alias (`FROM tb_post WHERE …`).
    if FROM_KEYWORDS.contains(&alias.to_lowercase().as_str()) {
        return None;
    }
    Some(alias)
}

/// Keep only `(col, key)` pairs where both the column and the key appear exactly
/// once across the whole map. A key claimed by two columns (`'x', a, 'x', b`) or a
/// column mapped to two keys is ambiguous ⇒ dropped ⇒ that column recomputes.
/// Insertion order is preserved.
fn drop_ambiguous(pairs: &[(String, String)]) -> Vec<(String, String)> {
    let mut key_counts: HashMap<&str, u32> = HashMap::new();
    let mut col_counts: HashMap<&str, u32> = HashMap::new();
    for (col, key) in pairs {
        *key_counts.entry(key.as_str()).or_insert(0) += 1;
        *col_counts.entry(col.as_str()).or_insert(0) += 1;
    }
    pairs
        .iter()
        .filter(|(col, key)| key_counts[key.as_str()] == 1 && col_counts[col.as_str()] == 1)
        .cloned()
        .collect()
}

/// A whole-word, case-insensitive search returning the byte offset of the match.
/// `haystack_lower` and `needle_lower` must already be lowercased.
fn find_word(haystack_lower: &str, needle_lower: &str, start: usize) -> Option<usize> {
    if needle_lower.is_empty() {
        return None;
    }
    let hay = haystack_lower.as_bytes();
    let needle = needle_lower.as_bytes();
    let nlen = needle.len();
    let mut i = start;
    while i + nlen <= hay.len() {
        if &hay[i..i + nlen] == needle {
            let before = i == 0 || !is_ident_byte(hay[i - 1]);
            let after = i + nlen >= hay.len() || !is_ident_byte(hay[i + nlen]);
            if before && after {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// `true` if `s` is a simple SQL identifier: leading letter/underscore, then
/// letters/digits/underscores.
fn is_simple_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    is_simple_ident_chars(s)
}

/// `true` if every character of `s` is a letter, digit, or underscore (non-empty).
fn is_simple_ident_chars(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

const fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: run extraction and return an ordered `Vec` of `(col, key)`.
    fn extract(sql: &str, base: &str) -> Vec<(String, String)> {
        extract_direct_column_map(sql, base)
    }

    // ---- Cycle 1: happy path ---------------------------------------------

    #[test]
    fn extracts_bare_column_and_omits_nested_object() {
        let sql = "SELECT pk_post, fk_user, \
             jsonb_build_object('title', title, 'author', v_user.data) AS data \
             FROM tb_post";
        assert_eq!(
            extract(sql, "tb_post"),
            vec![("title".to_string(), "title".to_string())]
        );
    }

    #[test]
    fn extracts_alias_qualified_base_column() {
        let sql = "SELECT pk_post, \
             jsonb_build_object('title', p.title) AS data \
             FROM tb_post p";
        assert_eq!(
            extract(sql, "tb_post"),
            vec![("title".to_string(), "title".to_string())]
        );
    }

    #[test]
    fn extracts_alias_qualified_with_as_keyword() {
        let sql = "SELECT pk_post, \
             jsonb_build_object('title', p.title) AS data \
             FROM tb_post AS p";
        assert_eq!(
            extract(sql, "tb_post"),
            vec![("title".to_string(), "title".to_string())]
        );
    }

    #[test]
    fn extracts_base_table_qualified_column() {
        let sql = "SELECT pk_post, \
             jsonb_build_object('title', tb_post.title) AS data \
             FROM tb_post";
        assert_eq!(
            extract(sql, "tb_post"),
            vec![("title".to_string(), "title".to_string())]
        );
    }

    #[test]
    fn key_differs_from_column_name() {
        let sql = "SELECT pk_post, \
             jsonb_build_object('headline', title) AS data \
             FROM tb_post";
        assert_eq!(
            extract(sql, "tb_post"),
            vec![("title".to_string(), "headline".to_string())]
        );
    }

    #[test]
    fn extracts_multiple_bare_columns_in_order() {
        let sql = "SELECT pk_user, \
             jsonb_build_object('name', name, 'bio', bio, 'age', age) AS data \
             FROM tb_user";
        assert_eq!(
            extract(sql, "tb_user"),
            vec![
                ("name".to_string(), "name".to_string()),
                ("bio".to_string(), "bio".to_string()),
                ("age".to_string(), "age".to_string()),
            ]
        );
    }

    // ---- Cycle 2: rejection cases ----------------------------------------

    #[test]
    fn rejects_concatenation_expression() {
        let sql = "SELECT pk_user, \
             jsonb_build_object('full_name', first_name || ' ' || last_name, 'bio', bio) AS data \
             FROM tb_user";
        // Only the bare `bio` survives; the concat expression is omitted.
        assert_eq!(
            extract(sql, "tb_user"),
            vec![("bio".to_string(), "bio".to_string())]
        );
    }

    #[test]
    fn rejects_cast_expression() {
        let sql = "SELECT pk_user, \
             jsonb_build_object('n', n::text, 'bio', bio) AS data \
             FROM tb_user";
        assert_eq!(
            extract(sql, "tb_user"),
            vec![("bio".to_string(), "bio".to_string())]
        );
    }

    #[test]
    fn rejects_other_relation_qualifier() {
        let sql = "SELECT pk_post, \
             jsonb_build_object('uname', u.name, 'title', title) AS data \
             FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user";
        // `u.name` is a joined relation → omitted; base `title` survives.
        assert_eq!(
            extract(sql, "tb_post"),
            vec![("title".to_string(), "title".to_string())]
        );
    }

    #[test]
    fn rejects_jsonb_agg_value() {
        let sql = "SELECT pk_user, \
             jsonb_build_object('name', name, 'posts', jsonb_agg(v_post.data)) AS data \
             FROM tb_user LEFT JOIN v_post ON v_post.fk_user = pk_user GROUP BY pk_user, name";
        assert_eq!(
            extract(sql, "tb_user"),
            vec![("name".to_string(), "name".to_string())]
        );
    }

    #[test]
    fn rejects_nested_jsonb_build_object_value() {
        let sql = "SELECT pk_post, \
             jsonb_build_object('title', title, 'meta', jsonb_build_object('x', 1)) AS data \
             FROM tb_post";
        assert_eq!(
            extract(sql, "tb_post"),
            vec![("title".to_string(), "title".to_string())]
        );
    }

    #[test]
    fn duplicate_key_drops_both() {
        let sql = "SELECT pk_post, \
             jsonb_build_object('x', a, 'x', b, 'title', title) AS data \
             FROM tb_post";
        // Both `x` mappings are ambiguous and dropped; only `title` remains.
        assert_eq!(
            extract(sql, "tb_post"),
            vec![("title".to_string(), "title".to_string())]
        );
    }

    #[test]
    fn duplicate_column_drops_both() {
        // Same column mapped to two keys — the col→key map can't represent this
        // faithfully, so both are dropped (recompute).
        let sql = "SELECT pk_post, \
             jsonb_build_object('a', title, 'b', title, 'body', body) AS data \
             FROM tb_post";
        assert_eq!(
            extract(sql, "tb_post"),
            vec![("body".to_string(), "body".to_string())]
        );
    }

    #[test]
    fn no_jsonb_build_object_yields_empty_map() {
        let sql = "SELECT pk_post, sub.data AS data FROM tb_post \
             JOIN (SELECT pk_post, data FROM other) sub USING (pk_post)";
        assert!(extract(sql, "tb_post").is_empty());
    }

    #[test]
    fn coalesce_wrapped_data_yields_empty_map() {
        let sql = "SELECT pk_post, \
             COALESCE(jsonb_build_object('title', title), '{}'::jsonb) AS data \
             FROM tb_post";
        // Not a bare jsonb_build_object() call → conservatively empty.
        assert!(extract(sql, "tb_post").is_empty());
    }

    #[test]
    fn missing_data_column_yields_empty_map() {
        let sql = "SELECT pk_post, title FROM tb_post";
        assert!(extract(sql, "tb_post").is_empty());
    }

    #[test]
    fn rejects_quoted_weird_key() {
        // Key with an embedded quote is not a simple identifier ⇒ omitted,
        // keeping ARRAY['…'] path building safe.
        let sql = "SELECT pk_post, \
             jsonb_build_object('title', title) AS data FROM tb_post";
        // Sanity: the safe key is kept.
        assert_eq!(
            extract(sql, "tb_post"),
            vec![("title".to_string(), "title".to_string())]
        );
    }
}

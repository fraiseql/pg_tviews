//! Keep TVIEW metadata in step with `ALTER … RENAME COLUMN` on a relation a
//! backing view reads from.
//!
//! `PostgreSQL` rewrites the backing view to follow a column rename, but the text in
//! `pg_tview_meta.definition` and everything derived from it at creation (cascade
//! paths, the column-aware refresh set, the direct-patch map, DISTINCT ON keys)
//! would still name the old column: propagation from the renamed column silently
//! stops. After a rename, every affected TVIEW's definition is rewritten and its
//! metadata re-derived with the same code that creation uses.
//!
//! The definition is rewritten in place, token by token, so it stays the author's
//! text. The rewrite is only kept if it defines exactly the same view as the
//! renamed backing view; otherwise the definition falls back to `pg_get_viewdef`.

use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};

/// Re-derive the metadata of every TVIEW whose backing view reads column
/// `new_name` (formerly `old_name`) of relation `relid`.
///
/// # Errors
/// Returns an error if a catalog query fails or the re-derived definition cannot
/// be analyzed; the caller aborts the `RENAME` so no TVIEW is left stale.
pub fn handle_column_rename(relid: Oid, old_name: &str, new_name: &str) -> TViewResult<()> {
    let affected = affected_tviews(relid, new_name)?;
    if !affected.is_empty() {
        crate::revision::check();
    }
    for (entity, schema_name, view_oid) in affected {
        let definition: String = Spi::get_one_with_args(
            &format!(
                "SELECT definition FROM {} WHERE entity = $1",
                crate::utils::meta_table()
            ),
            &[crate::utils::spi::text(&entity)],
        )
        .map_err(|e| crate::utils::spi::catalog_error("Read TVIEW definition", &e))?
        .unwrap_or_default();

        let relname = relation_name(relid)?;
        let rewritten = rewrite_column_references(&definition, &relname, old_name, new_name)
            // Only ever run as one SELECT: the rewrite works on tokens, and the
            // check executes the candidate.
            .filter(|candidate| crate::ddl::create::check_one_select(candidate).is_ok())
            .filter(|candidate| defines_view(candidate, view_oid));
        let new_definition = if let Some(sql) = rewritten {
            sql
        } else {
            let viewdef: String = Spi::get_one_with_args(
                "SELECT pg_get_viewdef($1)",
                &[crate::utils::spi::oid(view_oid)],
            )
            .map_err(|e| crate::utils::spi::catalog_error("Read backing view definition", &e))?
            .unwrap_or_default();
            notice!(
                "pg_tviews: definition of tv_{entity} re-rendered by PostgreSQL after renaming \
                 {relname}.{old_name}; the original text could not be rewritten in place"
            );
            viewdef.trim().trim_end_matches(';').to_string()
        };

        // An aggregate TVIEW names its group key columns by name. The
        // rename was authorized by PostgreSQL; the catalog is written as the
        // extension's owner.
        let owner = crate::owner::AsOwner::of_extension()?;
        Spi::run_with_args(
            &format!(
                "UPDATE {} SET group_keys = jsonb_set(group_keys, ARRAY[$2], to_jsonb($4)) \
                 WHERE entity = $1 AND group_keys->>$2 = $3",
                crate::utils::meta_table()
            ),
            &[
                crate::utils::spi::text(&entity),
                crate::utils::spi::text(&relname),
                crate::utils::spi::text(old_name),
                crate::utils::spi::text(new_name),
            ],
        )
        .map_err(|e| crate::utils::spi::catalog_error("Rename a group key column", &e))?;
        drop(owner);

        crate::ddl::create::reregister_metadata(&entity, &schema_name, &new_definition)?;
    }
    Ok(())
}

/// TVIEWs whose backing view's own rule depends on column `column` of `relid`
/// (the column-level `pg_depend` entries of its rewrite rule): the definitions
/// that name the column. A view the TVIEW reads follows the rename by itself.
fn affected_tviews(relid: Oid, column: &str) -> TViewResult<Vec<(String, String, Oid)>> {
    let query = format!(
        "{} SELECT DISTINCT m.entity, n.nspname::text AS schema, m.view_oid::oid AS view_oid \
         FROM reads r \
         JOIN {} m ON m.view_oid::oid = r.root \
         JOIN pg_catalog.pg_class t ON t.oid = m.table_oid \
         JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = r.relid AND a.attnum = r.attnum \
         WHERE r.depth = 0 AND r.relid = $1 AND a.attname = $2 \
         ORDER BY m.entity",
        crate::catalog::reads::view_reads_cte(&format!(
            "SELECT m.view_oid::pg_catalog.oid, m.view_oid::pg_catalog.oid, 0 FROM {} m",
            crate::utils::meta_table()
        )),
        crate::utils::meta_table()
    );
    Spi::connect(|client| {
        let args = [
            crate::utils::spi::oid(relid),
            crate::utils::spi::text(column),
        ];
        let mut out = Vec::new();
        for row in client.select(&query, None, &args)? {
            let entity: Option<String> = row["entity"].value()?;
            let schema: Option<String> = row["schema"].value()?;
            let view_oid: Option<Oid> = row["view_oid"].value()?;
            if let (Some(e), Some(s), Some(v)) = (entity, schema, view_oid) {
                out.push((e, s, v));
            }
        }
        Ok::<_, spi::Error>(out)
    })
    .map_err(|e| crate::utils::spi::catalog_error("Find TVIEWs reading the renamed column", &e))
}

fn relation_name(relid: Oid) -> TViewResult<String> {
    Spi::get_one_with_args::<String>(
        "SELECT relname::text FROM pg_class WHERE oid = $1",
        &[crate::utils::spi::oid(relid)],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Resolve renamed relation", &e))?
    .ok_or_else(|| TViewError::CatalogError {
        operation: "Resolve renamed relation".to_string(),
        pg_error: format!("relation {relid:?} not found"),
    })
}

/// Whether `candidate` defines the same view as `view_oid`, compared through
/// `pg_get_viewdef` (any error, e.g. a syntax error in the candidate, is false).
fn defines_view(candidate: &str, view_oid: Oid) -> bool {
    Spi::get_one_with_args::<bool>(
        &format!(
            "SELECT {}.pg_tviews_defines_view($1, $2)",
            crate::utils::ext_schema()
        ),
        &[
            crate::utils::spi::oid(view_oid),
            crate::utils::spi::text(candidate),
        ],
    )
    .ok()
    .flatten()
    .unwrap_or(false)
}

/// Rewrite the references to column `old` of table `table` in `sql` to `new`,
/// leaving every other byte of the text as written.
///
/// A reference is `<q>.old` where `<q>` is the table's name or one of its
/// aliases, or a bare `old` that is not itself a qualifier, a function name or
/// an alias. A reference that is a whole select-list item gets `AS old`, so the
/// output column keeps its name. Returns `None` if the text does not tokenize or
/// nothing matched. The result is a candidate: the caller must verify it.
#[must_use]
pub fn rewrite_column_references(sql: &str, table: &str, old: &str, new: &str) -> Option<String> {
    rewrite_with(sql, table, old, new, crate::utils::quote_ident)
}

/// [`rewrite_column_references`] with `quote` writing an identifier.
fn rewrite_with(
    sql: &str,
    table: &str,
    old: &str,
    new: &str,
    quote: impl Fn(&str) -> String,
) -> Option<String> {
    let tokens = Tokenizer::new(&PostgreSqlDialect {}, sql)
        .tokenize_with_location()
        .ok()?;
    let offsets = byte_offsets(sql, &tokens);
    // Indices of the non-whitespace tokens, for looking at neighbours.
    let sig: Vec<usize> = (0..tokens.len())
        .filter(|&i| !matches!(tokens[i].token, Token::Whitespace(_)))
        .collect();
    let tok = |k: usize| &tokens[sig[k]].token;
    let qualifiers = table_qualifiers(
        &sig.iter().map(|&i| &tokens[i].token).collect::<Vec<_>>(),
        table,
    );
    let select_items =
        select_list_items(&sig.iter().map(|&i| &tokens[i].token).collect::<Vec<_>>());

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for k in 0..sig.len() {
        let Token::Word(w) = tok(k) else { continue };
        if !ident_eq(w, old) {
            continue;
        }
        let prev = k.checked_sub(1).map(tok);
        let next = sig.get(k + 1).map(|&i| &tokens[i].token);
        if matches!(next, Some(Token::Period | Token::LParen)) {
            continue; // a qualifier or a function name
        }
        let start = if prev == Some(&Token::Period) {
            let Some(Token::Word(q)) = k.checked_sub(2).map(tok) else {
                continue;
            };
            if !qualifiers.iter().any(|name| ident_eq(q, name)) {
                continue;
            }
            k - 2
        } else {
            if matches!(prev, Some(Token::Word(p)) if p.keyword == Keyword::AS) {
                continue; // an alias named like the column
            }
            k
        };
        let (from, to) = (offsets[sig[k]], offsets[sig[k] + 1]);
        let mut replacement = quote(new);
        if select_items.contains(&(start, k)) {
            replacement.push_str(" AS ");
            replacement.push_str(&quote(old));
        }
        edits.push((from, to, replacement));
    }
    if edits.is_empty() {
        return None;
    }
    let mut out = sql.to_string();
    for (from, to, replacement) in edits.into_iter().rev() {
        out.replace_range(from..to, &replacement);
    }
    Some(out)
}

/// Byte offset of every token in `sql`, plus a final entry for the end of text.
pub(crate) fn byte_offsets(sql: &str, tokens: &[TokenWithSpan]) -> Vec<usize> {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(sql.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let mut offsets: Vec<usize> = tokens
        .iter()
        .map(|t| {
            let line = usize::try_from(t.span.start.line).unwrap_or(1).max(1) - 1;
            let col = usize::try_from(t.span.start.column).unwrap_or(1).max(1) - 1;
            let start = line_starts.get(line).copied().unwrap_or(sql.len());
            sql[start..]
                .char_indices()
                .nth(col)
                .map_or(sql.len(), |(i, _)| start + i)
        })
        .collect();
    offsets.push(sql.len());
    offsets
}

/// Names that qualify columns of `table`: the table name and any alias given to it.
fn table_qualifiers(tokens: &[&Token], table: &str) -> Vec<String> {
    let mut names = vec![table.to_string()];
    for (k, t) in tokens.iter().enumerate() {
        let Token::Word(w) = t else { continue };
        if !ident_eq(w, table) || matches!(tokens.get(k + 1), Some(Token::Period | Token::LParen)) {
            continue;
        }
        let mut j = k + 1;
        if matches!(tokens.get(j), Some(Token::Word(a)) if a.keyword == Keyword::AS) {
            j += 1;
        }
        if let Some(Token::Word(alias)) = tokens.get(j)
            && (alias.keyword == Keyword::NoKeyword || alias.quote_style.is_some())
        {
            names.push(alias.value.clone());
        }
    }
    names
}

/// `(first, last)` significant-token index pairs of select-list items that are a
/// plain, unaliased column reference (`col` or `q.col`).
fn select_list_items(tokens: &[&Token]) -> Vec<(usize, usize)> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    // Paren depths at which a SELECT list is open.
    let mut open_lists: Vec<usize> = Vec::new();
    for (k, t) in tokens.iter().enumerate() {
        match t {
            Token::LParen => depth += 1,
            Token::RParen => {
                open_lists.retain(|&d| d < depth);
                depth = depth.saturating_sub(1);
            }
            Token::Word(w) if w.keyword == Keyword::SELECT => open_lists.push(depth),
            Token::Word(w) if w.keyword == Keyword::FROM && open_lists.last() == Some(&depth) => {
                open_lists.pop();
            }
            _ => {}
        }
        if open_lists.last() != Some(&depth) {
            continue;
        }
        let starts_item = |i: usize| {
            matches!(tokens.get(i.wrapping_sub(1)), Some(Token::Comma))
                || matches!(tokens.get(i.wrapping_sub(1)), Some(Token::Word(w))
                    if matches!(w.keyword, Keyword::SELECT | Keyword::DISTINCT | Keyword::ALL))
        };
        let ends_item = |i: usize| {
            matches!(tokens.get(i + 1), Some(Token::Comma))
                || matches!(tokens.get(i + 1), Some(Token::Word(w)) if w.keyword == Keyword::FROM)
                || tokens.get(i + 1).is_none()
        };
        if !matches!(t, Token::Word(_)) || !ends_item(k) {
            continue;
        }
        if starts_item(k) {
            items.push((k, k));
        } else if k >= 2
            && matches!(tokens[k - 1], Token::Period)
            && matches!(tokens[k - 2], Token::Word(_))
            && starts_item(k - 2)
        {
            items.push((k - 2, k));
        }
    }
    items
}

/// Identifier equality with `PostgreSQL` folding: unquoted words compare
/// case-insensitively, quoted ones exactly.
fn ident_eq(word: &sqlparser::tokenizer::Word, name: &str) -> bool {
    if word.quote_style.is_some() {
        word.value == name
    } else {
        word.value.to_lowercase() == name
    }
}

#[cfg(test)]
mod tests {
    /// What `quote_ident()` writes for the identifiers of these tests (no keyword).
    fn quote_plain(name: &str) -> String {
        let plain = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if plain {
            name.to_string()
        } else {
            crate::utils::quote_identifier(name)
        }
    }

    fn rw(sql: &str, table: &str, old: &str, new: &str) -> Option<String> {
        super::rewrite_with(sql, table, old, new, quote_plain)
    }

    #[test]
    fn qualified_reference_inside_function_keeps_string_key() {
        assert_eq!(
            rw(
                "SELECT p.pk_post, jsonb_build_object('title', p.title) AS data FROM tb_post p",
                "tb_post",
                "title",
                "headline"
            )
            .as_deref(),
            Some(
                "SELECT p.pk_post, jsonb_build_object('title', p.headline) AS data FROM tb_post p"
            )
        );
    }

    #[test]
    fn bare_select_item_keeps_its_output_name() {
        assert_eq!(
            rw(
                "SELECT p.pk_post, p.fk_author, p.id FROM tb_post p",
                "tb_post",
                "fk_author",
                "fk_writer"
            )
            .as_deref(),
            Some("SELECT p.pk_post, p.fk_writer AS fk_author, p.id FROM tb_post p")
        );
    }

    #[test]
    fn unqualified_reference_and_alias_via_as() {
        assert_eq!(
            rw(
                "SELECT pk_a, jsonb_build_object('n', name) AS data FROM tb_a AS x WHERE x.name <> ''",
                "tb_a",
                "name",
                "full_name"
            )
            .as_deref(),
            Some(
                "SELECT pk_a, jsonb_build_object('n', full_name) AS data FROM tb_a AS x WHERE x.full_name <> ''"
            )
        );
    }

    #[test]
    fn other_tables_columns_and_aliases_are_untouched() {
        assert_eq!(
            rw(
                "SELECT p.pk_post, a.name AS title FROM tb_post p JOIN tb_author a ON a.pk_author = p.fk_author",
                "tb_post",
                "title",
                "headline"
            ),
            None
        );
    }

    #[test]
    fn multiline_offsets_and_quoted_new_name() {
        assert_eq!(
            rw(
                "SELECT p.pk_post,\n       jsonb_build_object('é', p.title) AS data\nFROM tb_post p",
                "tb_post",
                "title",
                "Head Line"
            )
            .as_deref(),
            Some(
                "SELECT p.pk_post,\n       jsonb_build_object('é', p.\"Head Line\") AS data\nFROM tb_post p"
            )
        );
    }

    #[test]
    fn comments_and_dollar_quoted_text_are_not_rewritten() {
        assert_eq!(
            rw(
                "SELECT p.title, -- p.title isn't renamed here\n  $$p.title$$ AS lit FROM tb_post p",
                "tb_post",
                "title",
                "headline"
            )
            .as_deref(),
            Some(
                "SELECT p.headline AS title, -- p.title isn't renamed here\n  $$p.title$$ AS lit FROM tb_post p"
            )
        );
    }
}

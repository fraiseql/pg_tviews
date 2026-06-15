/// Parse SELECT statement to extract column names and expressions
/// This is a simplified parser for v1 - uses regex-based extraction
/// Future versions will use `PostgreSQL`'s native parser API
///
/// # Errors
/// Returns error if SQL lacks SELECT/FROM keywords or has invalid syntax
pub fn parse_select_columns(sql: &str) -> Result<Vec<String>, String> {
    extract_columns_regex(sql)
}

/// Parse SELECT statement to extract column names with their full expressions
/// Returns `Vec<(column_name, expression)>` for type inference
///
/// # Errors
/// Returns error if SQL lacks SELECT/FROM keywords or has invalid syntax
pub fn parse_select_columns_with_expressions(sql: &str) -> Result<Vec<(String, String)>, String> {
    extract_columns_with_expressions_regex(sql)
}

/// Simple regex-based column extraction from SELECT statement
/// Limitations:
/// - Doesn't handle complex expressions
/// - Future: Replace with `PostgreSQL` parser API
fn extract_columns_regex(sql: &str) -> Result<Vec<String>, String> {
    let mut columns = Vec::new();

    // Normalize whitespace and case
    let sql_lower = sql.to_lowercase();

    // Skip CTE preamble (WITH ... AS (...)) if present
    let cte_offset = skip_cte_preamble(&sql_lower)?;

    // Find SELECT keyword starting from after any CTE preamble
    let select_start = sql_lower[cte_offset..]
        .find("select")
        .map(|p| p + cte_offset)
        .ok_or("No SELECT keyword found")?;

    // Bound the FROM search to the first branch of any UNION ALL / UNION
    let union_bound = find_outer_union(&sql_lower, select_start).unwrap_or(sql_lower.len());

    // Find the outermost FROM — skip FROMs inside parentheses (e.g., ARRAY subqueries)
    let from_start =
        find_outer_from(&sql_lower, select_start, union_bound).ok_or("No FROM keyword found")?;

    if from_start <= select_start {
        return Err("FROM appears before SELECT".to_string());
    }

    // Skip DISTINCT ON (...) or plain DISTINCT if present after SELECT
    let col_start = skip_distinct_clause(&sql_lower, select_start + 6);

    // Extract SELECT clause (columns only, after any DISTINCT ON)
    let select_clause = &sql[col_start..from_start].trim();

    if select_clause.is_empty() {
        return Err("No columns found in SELECT statement".to_string());
    }

    // Split by commas, respecting parentheses and quotes
    let parts = split_by_top_level_comma(select_clause);

    for part in parts {
        let trimmed = part.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Extract column name or alias
        let col_name = extract_column_name(trimmed)?;
        columns.push(col_name);
    }

    if columns.is_empty() {
        return Err("No columns found in SELECT statement".to_string());
    }

    Ok(columns)
}

/// Find the first occurrence of the `FROM` keyword at paren depth 0 (outermost level).
///
/// `sql_lower` must already be lowercased. Only looks in `after_pos..end_bound`.
/// Handles parentheses depth and single-quoted string literals.
fn find_outer_from(sql_lower: &str, after_pos: usize, end_bound: usize) -> Option<usize> {
    let bytes = sql_lower.as_bytes();
    let mut depth: i32 = 0;
    let mut i = after_pos;
    let len = bytes.len().min(end_bound);

    while i < len {
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'\'' => {
                // Skip single-quoted literal
                i += 1;
                while i < len && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < len {
                    i += 1;
                } // skip closing quote
            }
            _ => {
                // Check for "from" at word boundary, only at depth 0
                if depth == 0 && i + 4 <= len && &bytes[i..i + 4] == b"from" {
                    let before_ok =
                        i == 0 || (!bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_');
                    let after_ok = i + 4 >= len
                        || (!bytes[i + 4].is_ascii_alphanumeric() && bytes[i + 4] != b'_');
                    if before_ok && after_ok {
                        return Some(i);
                    }
                }
                i += 1;
            }
        }
    }
    None
}

/// Find the first occurrence of the `UNION` keyword at paren depth 0.
///
/// Returns the byte position of the `u` in `union` (lowercased), or `None` if
/// no outer UNION is found.  Both `UNION ALL` and `UNION` (deduplicated) are
/// detected; the caller uses the returned position as an upper bound for FROM
/// scanning so only the first branch's `FROM` is returned.
///
/// Public so callers outside this module (e.g. `ddl/create.rs`) can detect
/// whether a SQL statement is a UNION query for metadata purposes.
///
/// `sql_lower` must already be lowercased. Scans from `start`.
#[must_use]
pub fn find_outer_union(sql_lower: &str, start: usize) -> Option<usize> {
    let bytes = sql_lower.as_bytes();
    let len = bytes.len();
    let mut depth: i32 = 0;
    let mut i = start;

    while i < len {
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'\'' => {
                // Skip single-quoted literal
                i += 1;
                while i < len && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
            }
            b'"' => {
                // Skip double-quoted identifier
                i += 1;
                while i < len && bytes[i] != b'"' {
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
            }
            _ => {
                // Check for "union" at word boundary, only at depth 0
                if depth == 0 && i + 5 <= len && &bytes[i..i + 5] == b"union" {
                    let before_ok =
                        i == 0 || (!bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_');
                    let after_ok = i + 5 >= len
                        || (!bytes[i + 5].is_ascii_alphanumeric() && bytes[i + 5] != b'_');
                    if before_ok && after_ok {
                        return Some(i);
                    }
                }
                i += 1;
            }
        }
    }
    None
}

/// Skip a `DISTINCT ON (...)` or plain `DISTINCT` clause immediately following the
/// `SELECT` keyword and return the byte offset of the first column.
///
/// `sql_lower` must already be lowercased.
/// `after_select` is the byte offset just past the `SELECT` keyword (i.e. `select_start + 6`).
///
/// - `SELECT DISTINCT ON (c.id, c.ver) pk, id ...` → returns offset of `pk`
/// - `SELECT DISTINCT pk, id ...` → returns offset of `pk`
/// - `SELECT pk, id ...` → returns `after_select` unchanged (no-op)
fn skip_distinct_clause(sql_lower: &str, after_select: usize) -> usize {
    let bytes = sql_lower.as_bytes();
    let len = bytes.len();
    let mut i = after_select;

    // Skip whitespace
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    // Check for "distinct" keyword
    if i + 8 > len || &bytes[i..i + 8] != b"distinct" {
        return after_select; // no DISTINCT clause
    }
    let after_distinct = i + 8;
    if after_distinct < len
        && (bytes[after_distinct].is_ascii_alphanumeric() || bytes[after_distinct] == b'_')
    {
        return after_select; // not a keyword boundary
    }
    i += 8; // skip "distinct"

    // Skip whitespace after DISTINCT
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    // Check for "on" keyword (DISTINCT ON vs plain DISTINCT)
    if i + 2 <= len && &bytes[i..i + 2] == b"on" {
        let after_on = i + 2;
        if after_on >= len || (!bytes[after_on].is_ascii_alphanumeric() && bytes[after_on] != b'_')
        {
            i += 2; // skip "on"

            // Skip whitespace before "("
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }

            // Skip the parenthesized expression list
            if i < len
                && bytes[i] == b'('
                && let Ok(end) = skip_paren_block(bytes, i)
            {
                i = end;
            }
        }
        // If "on" is not a keyword boundary, it's a plain DISTINCT — already advanced past "distinct"
    }
    // else: plain DISTINCT (no ON) — already skipped "distinct"

    i
}

/// Extract the DISTINCT ON key expressions from a SELECT statement.
///
/// Returns the column/expression names listed inside `DISTINCT ON (...)`.
/// Strips table qualifiers (e.g. `c.id` → `"id"`).
/// Returns an empty `Vec` if the SQL does not use `DISTINCT ON`.
///
/// # Errors
///
/// Returns `Err` if the SQL cannot be parsed (passed through from `skip_cte_preamble`).
pub fn extract_distinct_on_keys(sql: &str) -> Result<Vec<String>, String> {
    let sql_lower = sql.to_lowercase();
    let cte_offset = skip_cte_preamble(&sql_lower)?;

    let select_start = sql_lower[cte_offset..]
        .find("select")
        .map(|p| p + cte_offset)
        .ok_or("No SELECT keyword found")?;

    let bytes = sql_lower.as_bytes();
    let len = bytes.len();
    let mut i = select_start + 6;

    // Skip whitespace
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    // Must see "distinct"
    if i + 8 > len || &bytes[i..i + 8] != b"distinct" {
        return Ok(vec![]);
    }
    let after_distinct = i + 8;
    if after_distinct < len
        && (bytes[after_distinct].is_ascii_alphanumeric() || bytes[after_distinct] == b'_')
    {
        return Ok(vec![]);
    }
    i += 8;

    // Skip whitespace
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    // Must see "on"
    if i + 2 > len || &bytes[i..i + 2] != b"on" {
        return Ok(vec![]); // plain DISTINCT, no dedup keys
    }
    let after_on = i + 2;
    if after_on < len && (bytes[after_on].is_ascii_alphanumeric() || bytes[after_on] == b'_') {
        return Ok(vec![]); // "on" is part of an identifier
    }
    i += 2;

    // Skip whitespace
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    if i >= len || bytes[i] != b'(' {
        return Ok(vec![]);
    }

    // Find closing paren position
    let close = skip_paren_block(bytes, i)?;
    // Extract content between ( and )
    let inner = &sql[i + 1..close - 1].trim();

    // Split by top-level commas and strip table qualifiers
    let keys = split_by_top_level_comma(inner)
        .into_iter()
        .map(|k| {
            let trimmed = k.trim().to_string();
            // Strip table qualifier: "c.id" → "id"
            trimmed
                .split('.')
                .next_back()
                .unwrap_or(&trimmed)
                .trim()
                .to_string()
        })
        .filter(|k| !k.is_empty())
        .collect();

    Ok(keys)
}

/// Skip a leading `WITH` clause (CTE preamble) and return the byte offset into
/// `sql_lower` where the main SELECT starts.
///
/// Returns `Ok(0)` when the SQL does not begin with a `WITH` keyword.
/// Returns `Err(msg)` if `WITH RECURSIVE` is detected or the preamble is malformed.
///
/// `sql_lower` must already be lowercased.
fn skip_cte_preamble(sql_lower: &str) -> Result<usize, String> {
    let bytes = sql_lower.as_bytes();
    let len = bytes.len();

    // Skip leading whitespace
    let mut i = 0;
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    // Check for "with" keyword (requires word boundary after it)
    if i + 4 > len || &bytes[i..i + 4] != b"with" {
        return Ok(0);
    }
    let after_with = i + 4;
    if after_with < len && (bytes[after_with].is_ascii_alphanumeric() || bytes[after_with] == b'_')
    {
        return Ok(0); // e.g. "without", "within" — not a WITH keyword
    }

    i += 4; // skip "with"

    // Skip whitespace before first CTE name (or RECURSIVE keyword)
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    // Reject WITH RECURSIVE
    if i + 9 <= len && &bytes[i..i + 9] == b"recursive" {
        let after_rec = i + 9;
        if after_rec >= len
            || (!bytes[after_rec].is_ascii_alphanumeric() && bytes[after_rec] != b'_')
        {
            return Err("WITH RECURSIVE is not supported in TVIEWs. \
                 Consider using a non-recursive CTE or a subquery."
                .to_string());
        }
    }

    // Walk the CTE list: name [column_list] AS (body) [, ...]
    loop {
        // Skip whitespace before CTE name
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }

        if i >= len {
            return Err("Unexpected end of SQL while parsing CTE preamble".to_string());
        }

        // Skip CTE name: quoted or unquoted identifier
        if bytes[i] == b'"' {
            i += 1;
            while i < len && bytes[i] != b'"' {
                i += 1;
            }
            if i >= len {
                return Err("Unterminated quoted identifier in CTE name".to_string());
            }
            i += 1; // skip closing "
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
        } else {
            // Not an identifier — the main SELECT starts here
            return Ok(i);
        }

        // Skip whitespace
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }

        // Optional explicit column list: cte_name(col1, col2)
        if i < len && bytes[i] == b'(' {
            i = skip_paren_block(bytes, i)?;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
        }

        // Expect "as" keyword
        if i + 2 > len || &bytes[i..i + 2] != b"as" {
            return Err(format!(
                "Expected AS keyword in CTE definition at byte offset {i}"
            ));
        }
        let after_as = i + 2;
        if after_as < len && (bytes[after_as].is_ascii_alphanumeric() || bytes[after_as] == b'_') {
            return Err(format!(
                "Expected AS keyword in CTE definition at byte offset {i}"
            ));
        }
        i += 2; // skip "as"

        // Skip whitespace before opening paren
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }

        // Expect "(" opening the CTE body
        if i >= len || bytes[i] != b'(' {
            return Err(format!("Expected '(' for CTE body at byte offset {i}"));
        }

        i = skip_paren_block(bytes, i)?;

        // Skip whitespace after CTE body
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }

        if i >= len {
            return Err("SQL ends after CTE body with no main SELECT".to_string());
        }

        if bytes[i] == b',' {
            i += 1; // another CTE follows
        } else {
            // Main SELECT starts here
            return Ok(i);
        }
    }
}

/// Walk `bytes` starting at `start` (which must be `(`) to the matching `)`.
/// Handles nested parens, single-quoted strings (`''` escape), and double-quoted identifiers.
/// Returns the byte position **after** the closing `)`.
fn skip_paren_block(bytes: &[u8], start: usize) -> Result<usize, String> {
    let len = bytes.len();
    let mut i = start;
    let mut depth: i32 = 0;

    while i < len {
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            b'\'' => {
                // Single-quoted string; handle '' escape sequence
                i += 1;
                loop {
                    if i >= len {
                        break;
                    }
                    if bytes[i] == b'\'' {
                        i += 1;
                        if i < len && bytes[i] == b'\'' {
                            i += 1; // escaped quote — continue
                        } else {
                            break; // end of string literal
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'"' => {
                // Double-quoted identifier
                i += 1;
                while i < len && bytes[i] != b'"' {
                    i += 1;
                }
                if i < len {
                    i += 1; // skip closing "
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    Err("Unbalanced parentheses in SQL".to_string())
}

/// Extract columns with their full expressions from SELECT statement
fn extract_columns_with_expressions_regex(sql: &str) -> Result<Vec<(String, String)>, String> {
    let mut columns = Vec::new();

    // Normalize whitespace and case
    let sql_lower = sql.to_lowercase();

    // Skip CTE preamble (WITH ... AS (...)) if present
    let cte_offset = skip_cte_preamble(&sql_lower)?;

    // Find SELECT keyword starting from after any CTE preamble
    let select_start = sql_lower[cte_offset..]
        .find("select")
        .map(|p| p + cte_offset)
        .ok_or("No SELECT keyword found")?;

    // Bound the FROM search to the first branch of any UNION ALL / UNION
    let union_bound = find_outer_union(&sql_lower, select_start).unwrap_or(sql_lower.len());

    // Find the outermost FROM — skip FROMs inside parentheses (e.g., ARRAY subqueries)
    let from_start =
        find_outer_from(&sql_lower, select_start, union_bound).ok_or("No FROM keyword found")?;

    if from_start <= select_start {
        return Err("FROM appears before SELECT".to_string());
    }

    // Skip DISTINCT ON (...) or plain DISTINCT if present after SELECT
    let col_start = skip_distinct_clause(&sql_lower, select_start + 6);

    // Extract SELECT clause (columns only, after any DISTINCT ON)
    let select_clause = &sql[col_start..from_start].trim();

    if select_clause.is_empty() {
        return Err("No columns found in SELECT statement".to_string());
    }

    // Split by commas, respecting parentheses and quotes
    let parts = split_by_top_level_comma(select_clause);

    for part in parts {
        let trimmed = part.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Extract column name and keep full expression
        let col_name = extract_column_name(trimmed)?;
        columns.push((col_name, trimmed.to_string()));
    }

    if columns.is_empty() {
        return Err("No columns found in SELECT statement".to_string());
    }

    Ok(columns)
}

/// Split string by commas, but only at top level (outside parentheses and quotes)
fn split_by_top_level_comma(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut paren_depth: i32 = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut prev_char = '\0';

    for c in s.chars() {
        match c {
            '(' if !in_single_quote && !in_double_quote => {
                paren_depth += 1;
                current.push(c);
            }
            ')' if !in_single_quote && !in_double_quote => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(c);
            }
            '\'' if !in_double_quote => {
                // Toggle single quote state
                if prev_char != '\\' {
                    in_single_quote = !in_single_quote;
                }
                current.push(c);
            }
            '"' if !in_single_quote => {
                // Toggle double quote state (handle escaping)
                if prev_char != '\\' {
                    in_double_quote = !in_double_quote;
                }
                current.push(c);
            }
            ',' if paren_depth == 0 && !in_single_quote && !in_double_quote => {
                // Top-level comma - split here
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
        prev_char = c;
    }

    // Push the last part
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    parts
}

/// Extract column name from a SELECT clause part
/// Handles: `column_name`, `table.column_name`, `expression AS alias`
fn extract_column_name(part: &str) -> Result<String, String> {
    let part_lower = part.to_lowercase();

    // Check for `AS` keyword (alias)
    if let Some(as_pos) = find_last_as(&part_lower) {
        let alias_part = &part[as_pos + 2..].trim();
        if alias_part.is_empty() {
            return Err("Empty alias after AS".to_string());
        }
        return Ok((*alias_part).to_string());
    }

    // No alias - extract column name from expression
    // This is simplified - just take the last identifier
    let words: Vec<&str> = part.split_whitespace().collect();
    if words.is_empty() {
        return Err("Empty column expression".to_string());
    }

    // Take the last word (should be the column name)
    let last_word = words
        .last()
        .ok_or_else(|| "Unexpected empty words vector".to_string())?;

    // Remove trailing punctuation
    let clean_name = last_word.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');

    if clean_name.is_empty() {
        return Err("Could not extract column name".to_string());
    }

    // Strip table alias prefix: "a.id" → "id", "schema.table.col" → "col"
    // PostgreSQL uses the rightmost segment as the output column name when no AS alias.
    let col = clean_name.split('.').next_back().unwrap_or(clean_name);

    Ok(col.to_string())
}

/// Find the last `AS` keyword position, handling nested contexts
fn find_last_as(sql_lower: &str) -> Option<usize> {
    let mut last_as_pos = None;
    let bytes = sql_lower.as_bytes();

    for (i, _) in sql_lower.match_indices("as") {
        // Word-boundary check: "as" must be preceded by whitespace or start of string
        let preceded_by_space = i == 0 || bytes[i - 1].is_ascii_whitespace();
        // Word-boundary check: "as" must be followed by whitespace or end of string
        let followed_by_space = i + 2 >= sql_lower.len() || bytes[i + 2].is_ascii_whitespace();

        if !preceded_by_space || !followed_by_space {
            continue;
        }

        // Count parentheses to handle nested expressions
        let before = &sql_lower[..i];
        let paren_depth = before.chars().fold(0i32, |depth, c| match c {
            '(' => depth + 1,
            ')' => depth.saturating_sub(1),
            _ => depth,
        });

        // Only consider AS at top level (not inside parentheses)
        if paren_depth == 0 {
            last_as_pos = Some(i);
        }
    }

    last_as_pos
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── existing tests ────────────────────────────────────────────────────────

    #[test]
    fn test_extract_columns_simple() {
        let sql = "SELECT id, name, data FROM users";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["id", "name", "data"]);
    }

    #[test]
    fn test_extract_columns_with_alias() {
        let sql = "SELECT u.id AS user_id, u.name, 'literal' AS data FROM users u";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["user_id", "name", "data"]);
    }

    #[test]
    fn test_extract_columns_table_qualified() {
        let sql = "SELECT u.id, u.name, p.title FROM users u JOIN posts p ON u.id = p.user_id";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["id", "name", "title"]);
    }

    #[test]
    fn test_extract_columns_complex_expression() {
        let sql =
            "SELECT pk_post, id, jsonb_build_object('id', id, 'title', title) AS data FROM posts";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_post", "id", "data"]);
    }

    #[test]
    fn test_extract_columns_empty_select() {
        let sql = "SELECT FROM users";
        let result = parse_select_columns(sql);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No columns"));
    }

    #[test]
    fn test_extract_columns_no_select() {
        // "FROM" before "SELECT" — find_outer_from starts after "select", finds nothing
        let sql = "FROM users SELECT id";
        let result = parse_select_columns(sql);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No FROM keyword found"));
    }

    #[test]
    fn test_extract_column_name_simple() {
        assert_eq!(extract_column_name("id").unwrap(), "id");
        assert_eq!(extract_column_name("pk_post").unwrap(), "pk_post");
        assert_eq!(extract_column_name("u.name").unwrap(), "name");
    }

    #[test]
    fn test_extract_column_name_with_alias() {
        assert_eq!(extract_column_name("u.id AS user_id").unwrap(), "user_id");
        assert_eq!(
            extract_column_name("jsonb_build_object('key', 'value') AS data").unwrap(),
            "data"
        );
    }

    #[test]
    fn test_find_last_as() {
        // find_last_as operates on already-lowercased strings
        assert_eq!(find_last_as("id as user_id"), Some(3));
        assert_eq!(
            find_last_as("jsonb_build_object('id', id) as data"),
            Some(29)
        );
        assert_eq!(find_last_as("id"), None);
    }

    #[test]
    fn test_find_last_as_nested() {
        // "as" inside function call (depth > 0) should be ignored;
        // the last top-level "as" is the one we want
        let sql = "jsonb_build_object('id', id) as data, name as full_name";
        assert_eq!(find_last_as(sql), Some(43)); // Position of "as full_name"
    }

    #[test]
    fn test_parse_cte_columns() {
        let sql = "WITH labels AS (SELECT item_id, label FROM tb_i18n) \
                   SELECT i.pk_item, i.id, i.name, l.label AS data \
                   FROM tb_item i LEFT JOIN labels l ON l.item_id = i.pk_item";
        let cols = parse_select_columns(sql).unwrap();
        assert!(
            cols.contains(&"pk_item".to_string()),
            "expected pk_item, got {cols:?}"
        );
        assert!(
            cols.contains(&"id".to_string()),
            "expected id, got {cols:?}"
        );
        assert!(
            cols.contains(&"name".to_string()),
            "expected name, got {cols:?}"
        );
        assert!(
            cols.contains(&"data".to_string()),
            "expected data, got {cols:?}"
        );
        assert!(
            !cols.contains(&"item_id".to_string()),
            "CTE-only column item_id leaked: {cols:?}"
        );
        assert!(
            !cols.contains(&"label".to_string()),
            "CTE-only column label leaked: {cols:?}"
        );
    }

    #[test]
    fn test_parse_cte_columns_with_expressions() {
        let sql = "WITH labels AS (SELECT item_id, label FROM tb_i18n) \
                   SELECT i.pk_item, i.id, i.name, l.label AS data \
                   FROM tb_item i LEFT JOIN labels l ON l.item_id = i.pk_item";
        let cols = parse_select_columns_with_expressions(sql).unwrap();
        let names: Vec<&str> = cols.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"pk_item"),
            "expected pk_item, got {names:?}"
        );
        assert!(names.contains(&"data"), "expected data, got {names:?}");
        assert!(
            !names.contains(&"item_id"),
            "CTE-only column item_id leaked: {names:?}"
        );
    }

    #[test]
    fn test_parse_multiple_ctes() {
        let sql = "WITH a AS (SELECT x FROM t1), b AS (SELECT y FROM a) \
                   SELECT pk_item, id, b.y AS data FROM tb_item JOIN b ON b.y = tb_item.pk_item";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_item", "id", "data"]);
    }

    #[test]
    fn test_parse_cte_with_explicit_column_list() {
        // WITH cte(col1, col2) AS (...)
        let sql = "WITH labeled(item_id, label) AS (SELECT item_id, label FROM tb_i18n) \
                   SELECT pk_item, id, label AS data FROM tb_item \
                   JOIN labeled ON labeled.item_id = tb_item.pk_item";
        let cols = parse_select_columns(sql).unwrap();
        assert!(cols.contains(&"pk_item".to_string()));
        assert!(cols.contains(&"data".to_string()));
        assert!(!cols.contains(&"item_id".to_string()));
    }

    #[test]
    fn test_parse_cte_with_paren_in_string_literal() {
        // CTE body contains a string with ')' inside — must not confuse paren counter
        let sql = "WITH filtered AS (SELECT id FROM t WHERE name = 'a)b') \
                   SELECT pk_item, id, name AS data FROM tb_item";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_item", "id", "data"]);
    }

    #[test]
    fn test_parse_cte_with_nested_subquery() {
        // CTE body contains a nested subquery
        let sql = "WITH top AS (SELECT id FROM t WHERE id IN (SELECT id FROM s)) \
                   SELECT pk_item, id, name AS data FROM tb_item";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_item", "id", "data"]);
    }

    #[test]
    fn test_recursive_cte_rejected() {
        let sql = "WITH RECURSIVE tree AS (SELECT 1) SELECT pk_item, id, name AS data FROM tb_item";
        let result = parse_select_columns(sql);
        assert!(result.is_err(), "expected error for WITH RECURSIVE");
        assert!(
            result.unwrap_err().contains("RECURSIVE"),
            "error should mention RECURSIVE"
        );
    }

    #[test]
    fn test_recursive_cte_rejected_lowercase() {
        let sql = "with recursive tree as (select 1) select pk_item, id, name as data from tb_item";
        let result = parse_select_columns(sql);
        assert!(result.is_err(), "expected error for with recursive");
        assert!(result.unwrap_err().contains("RECURSIVE"));
    }

    #[test]
    fn test_skip_paren_block_simple() {
        let s = "(hello world) rest";
        let end = skip_paren_block(s.as_bytes(), 0).unwrap();
        assert_eq!(end, 13); // position after ')'
    }

    #[test]
    fn test_skip_paren_block_nested() {
        let s = "((a) (b)) rest";
        let end = skip_paren_block(s.as_bytes(), 0).unwrap();
        assert_eq!(end, 9);
    }

    #[test]
    fn test_skip_paren_block_string_with_paren() {
        let s = "(WHERE name = 'a)b') rest";
        let end = skip_paren_block(s.as_bytes(), 0).unwrap();
        assert_eq!(end, 20);
    }

    #[test]
    fn test_non_cte_sql_unchanged() {
        let sql = "SELECT pk_post, id, jsonb_build_object('id', id) AS data FROM tb_post";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_post", "id", "data"]);
    }

    #[test]
    fn test_parse_distinct_on_columns() {
        let sql = "SELECT DISTINCT ON (c.id) c.pk_contract, c.id, c.name, \
                   jsonb_build_object('id', c.id, 'name', c.name) AS data \
                   FROM tenant.tb_contract c ORDER BY c.id, c.version DESC";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(
            cols,
            vec!["pk_contract", "id", "name", "data"],
            "got: {cols:?}"
        );
    }

    #[test]
    fn test_parse_distinct_on_columns_with_expressions() {
        let sql = "SELECT DISTINCT ON (c.id) c.pk_contract, c.id, c.name, \
                   jsonb_build_object('id', c.id) AS data \
                   FROM tenant.tb_contract c ORDER BY c.id, c.version DESC";
        let cols = parse_select_columns_with_expressions(sql).unwrap();
        let names: Vec<&str> = cols.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"pk_contract"), "got: {names:?}");
        assert!(names.contains(&"data"), "got: {names:?}");
    }

    #[test]
    fn test_parse_plain_distinct_columns() {
        // SELECT DISTINCT (no ON) — should also work
        let sql = "SELECT DISTINCT pk_item, id, name AS data FROM tb_item";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_item", "id", "data"]);
    }

    #[test]
    fn test_parse_composite_distinct_on() {
        let sql = "SELECT DISTINCT ON (c.tenant_id, c.id) c.pk_contract, c.id, c.name AS data \
                   FROM tb_contract c ORDER BY c.tenant_id, c.id, c.version DESC";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_contract", "id", "data"]);
    }

    #[test]
    fn test_extract_distinct_on_keys_single() {
        let sql = "SELECT DISTINCT ON (c.id) c.pk_contract, c.id FROM tb_contract c";
        let keys = extract_distinct_on_keys(sql).unwrap();
        assert_eq!(keys, vec!["id"]);
    }

    #[test]
    fn test_extract_distinct_on_keys_composite() {
        let sql = "SELECT DISTINCT ON (c.tenant_id, c.id) c.pk_contract, c.id FROM tb_contract c";
        let keys = extract_distinct_on_keys(sql).unwrap();
        assert_eq!(keys, vec!["tenant_id", "id"]);
    }

    #[test]
    fn test_extract_distinct_on_keys_none() {
        let sql = "SELECT pk_post, id, name FROM tb_post";
        let keys = extract_distinct_on_keys(sql).unwrap();
        assert!(keys.is_empty(), "expected no keys, got {keys:?}");
    }

    #[test]
    fn test_extract_distinct_on_keys_plain_distinct() {
        // SELECT DISTINCT (no ON) → no distinct_on_keys
        let sql = "SELECT DISTINCT pk_post, id FROM tb_post";
        let keys = extract_distinct_on_keys(sql).unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn test_extract_distinct_on_keys_with_cte() {
        let sql = "WITH cte AS (SELECT 1) SELECT DISTINCT ON (c.id) c.pk_contract, c.id FROM tb_contract c";
        let keys = extract_distinct_on_keys(sql).unwrap();
        assert_eq!(keys, vec!["id"]);
    }

    #[test]
    fn test_non_distinct_sql_unchanged() {
        let sql = "SELECT pk_post, id, data FROM tb_post JOIN tb_other ON TRUE";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_post", "id", "data"]);
    }

    #[test]
    fn test_parse_union_all_columns() {
        let sql = "SELECT i.pk_item, i.id, l.label AS name, \
                   jsonb_build_object('id', i.id, 'name', l.label) AS data \
                   FROM catalog.tb_item i \
                   JOIN catalog.tb_item_i18n l ON l.item_id = i.pk_item \
                   WHERE l.locale = 'en' \
                   UNION ALL \
                   SELECT i.pk_item, i.id, i.name, \
                   jsonb_build_object('id', i.id, 'name', i.name) AS data \
                   FROM catalog.tb_item i \
                   WHERE NOT EXISTS (SELECT 1 FROM catalog.tb_item_i18n l \
                   WHERE l.item_id = i.pk_item AND l.locale = 'en')";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_item", "id", "name", "data"]);
    }

    #[test]
    fn test_parse_union_deduplicated_columns() {
        let sql = "SELECT pk_post, id, title AS name FROM tb_post \
                   UNION \
                   SELECT pk_post, id, slug AS name FROM tb_post_draft";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_post", "id", "name"]);
    }

    #[test]
    fn test_union_in_string_literal_not_matched() {
        // 'UNION ALL' inside a string literal must NOT trigger UNION detection
        let sql = "SELECT pk_post, id, 'UNION ALL rocks' AS label FROM tb_post";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_post", "id", "label"]);
    }

    #[test]
    fn test_union_inside_subquery_not_matched() {
        // UNION inside a subquery (depth > 0) must NOT trigger outer-UNION detection
        let sql = "SELECT pk_post, id, (SELECT MAX(v) FROM (SELECT 1 AS v UNION SELECT 2 AS v) s) AS top \
                   FROM tb_post";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_post", "id", "top"]);
    }

    #[test]
    fn test_parse_cte_with_union_all() {
        let sql = "WITH fallback AS (SELECT pk_item, id, name FROM tb_item) \
                   SELECT pk_item, id, name AS label FROM tb_item \
                   UNION ALL \
                   SELECT pk_item, id, name AS label FROM fallback";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols, vec!["pk_item", "id", "label"]);
    }

    #[test]
    fn test_parse_distinct_on_union_all() {
        let sql = "SELECT DISTINCT ON (c.id) c.pk_contract, c.id, c.name, \
                   jsonb_build_object('id', c.id) AS data \
                   FROM tb_contract c ORDER BY c.id, c.version DESC \
                   UNION ALL \
                   SELECT pk_draft, id, name, \
                   jsonb_build_object('id', id) AS data \
                   FROM tb_contract_draft";
        let cols = parse_select_columns(sql).unwrap();
        assert_eq!(cols[0], "pk_contract");
        assert_eq!(cols[1], "id");
        assert_eq!(cols[2], "name");
        assert_eq!(cols[3], "data");
    }
}

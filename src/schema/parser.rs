

/// Parse SELECT statement to extract column names and expressions
/// This is a simplified parser for v1 - uses regex-based extraction
/// Future versions will use PostgreSQL's native parser API
pub fn parse_select_columns(sql: &str) -> Result<Vec<String>, String> {
    extract_columns_regex(sql)
}

/// Parse SELECT statement to extract column names with their full expressions
/// Returns Vec<(column_name, expression)> for type inference
pub fn parse_select_columns_with_expressions(sql: &str) -> Result<Vec<(String, String)>, String> {
    extract_columns_with_expressions_regex(sql)
}

/// Simple regex-based column extraction from SELECT statement
/// Limitations:
/// - Doesn't handle nested commas in function calls
/// - Doesn't handle complex expressions
/// - Doesn't handle subqueries properly
/// - Future: Replace with PostgreSQL parser API
fn extract_columns_regex(sql: &str) -> Result<Vec<String>, String> {
    let mut columns = Vec::new();

    // Normalize whitespace and case
    let sql_lower = sql.to_lowercase();

    // Find SELECT and FROM positions
    let select_start = sql_lower.find("select")
        .ok_or("No SELECT keyword found")?;
    let from_start = sql_lower.find("from")
        .ok_or("No FROM keyword found")?;

    if from_start <= select_start {
        return Err("FROM appears before SELECT".to_string());
    }

    // Extract SELECT clause
    let select_clause = &sql[select_start + 6..from_start].trim();

    if select_clause.is_empty() {
        return Err("Empty SELECT clause".to_string());
    }

    // Split by commas, respecting parentheses and quotes
    let parts = split_by_top_level_comma(select_clause)?;

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

/// Extract columns with their full expressions from SELECT statement
fn extract_columns_with_expressions_regex(sql: &str) -> Result<Vec<(String, String)>, String> {
    let mut columns = Vec::new();

    // Normalize whitespace and case
    let sql_lower = sql.to_lowercase();

    // Find SELECT and FROM positions
    let select_start = sql_lower.find("select")
        .ok_or("No SELECT keyword found")?;
    let from_start = sql_lower.find("from")
        .ok_or("No FROM keyword found")?;

    if from_start <= select_start {
        return Err("FROM appears before SELECT".to_string());
    }

    // Extract SELECT clause
    let select_clause = &sql[select_start + 6..from_start].trim();

    if select_clause.is_empty() {
        return Err("Empty SELECT clause".to_string());
    }

    // Split by commas, respecting parentheses and quotes
    let parts = split_by_top_level_comma(select_clause)?;

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
fn split_by_top_level_comma(s: &str) -> Result<Vec<String>, String> {
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
                // Toggle single quote state (handle escaping)
                if prev_char == '\\' {
                    current.push(c);
                } else {
                    in_single_quote = !in_single_quote;
                    current.push(c);
                }
            }
            '"' if !in_single_quote => {
                // Toggle double quote state (handle escaping)
                if prev_char == '\\' {
                    current.push(c);
                } else {
                    in_double_quote = !in_double_quote;
                    current.push(c);
                }
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

    Ok(parts)
}

/// Extract column name from a SELECT clause part
/// Handles: column_name, table.column_name, expression AS alias
fn extract_column_name(part: &str) -> Result<String, String> {
    let part_lower = part.to_lowercase();

    // Check for AS keyword (alias)
    if let Some(as_pos) = find_last_as(&part_lower) {
        let alias_part = &part[as_pos + 2..].trim();
        if alias_part.is_empty() {
            return Err("Empty alias after AS".to_string());
        }
        return Ok(alias_part.to_string());
    }

    // No alias - extract column name from expression
    // This is simplified - just take the last identifier
    let words: Vec<&str> = part.split_whitespace().collect();
    if words.is_empty() {
        return Err("Empty column expression".to_string());
    }

    // Take the last word (should be the column name)
    let last_word = words.last()
        .ok_or_else(|| "Unexpected empty words vector".to_string())?;

    // Remove trailing punctuation
    let clean_name = last_word.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');

    if clean_name.is_empty() {
        return Err("Could not extract column name".to_string());
    }

    Ok(clean_name.to_string())
}

/// Find the last "AS" keyword position, handling nested contexts
fn find_last_as(sql_lower: &str) -> Option<usize> {
    let mut last_as_pos = None;

    for (i, _) in sql_lower.match_indices("as") {
        // Count parentheses to handle nested expressions
        let before = &sql_lower[..i];
        let paren_depth = before.chars().fold(0i32, |depth, c| {
            match c {
                '(' => depth + 1,
                ')' => depth.saturating_sub(1),
                _ => depth,
            }
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
        let sql = "SELECT pk_post, id, jsonb_build_object('id', id, 'title', title) AS data FROM posts";
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
        let sql = "FROM users SELECT id";
        let result = parse_select_columns(sql);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("FROM appears before SELECT"));
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
        assert_eq!(extract_column_name("jsonb_build_object('key', 'value') AS data").unwrap(), "data");
    }

    #[test]
    fn test_find_last_as() {
        assert_eq!(find_last_as("id AS user_id"), Some(3));
        assert_eq!(find_last_as("jsonb_build_object('id', id) AS data"), Some(32));
        assert_eq!(find_last_as("id"), None);
    }

    #[test]
    fn test_find_last_as_nested() {
        // AS inside function call should be ignored
        let sql = "jsonb_build_object('id', id) AS data, name AS full_name";
        assert_eq!(find_last_as(sql), Some(40)); // Position of "AS full_name"
    }
}
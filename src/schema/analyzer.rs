use crate::catalog::DependencyType;
use regex::Regex;

/// Information about a detected dependency
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyInfo {
    /// Type of dependency (Scalar, NestedObject, Array)
    pub dep_type: DependencyType,
    /// JSONB path to the nested data (e.g., ["author"], ["comments"])
    pub jsonb_path: Option<Vec<String>>,
    /// For arrays, the key used to match elements (e.g., "id")
    pub array_match_key: Option<String>,
}

impl DependencyInfo {
    /// Create a scalar dependency (default)
    fn scalar() -> Self {
        Self {
            dep_type: DependencyType::Scalar,
            jsonb_path: None,
            array_match_key: None,
        }
    }

    /// Create a nested object dependency
    fn nested_object(path: String) -> Self {
        Self {
            dep_type: DependencyType::NestedObject,
            jsonb_path: Some(vec![path]),
            array_match_key: None,
        }
    }

    /// Create an array dependency
    fn array(path: String, match_key: String) -> Self {
        Self {
            dep_type: DependencyType::Array,
            jsonb_path: Some(vec![path]),
            array_match_key: Some(match_key),
        }
    }
}

/// Analyze SELECT statement to detect dependency types
///
/// # Arguments
/// * `select_sql` - The SELECT statement defining the TVIEW
/// * `fk_columns` - List of FK column names from schema inference
///
/// # Returns
/// Vector of DependencyInfo, one per FK column (order matches input)
pub fn analyze_dependencies(
    select_sql: &str,
    fk_columns: &[String],
) -> Vec<DependencyInfo> {
    let mut deps = Vec::new();

    for fk_col in fk_columns {
        let dep_info = detect_dependency_type(select_sql, fk_col);
        deps.push(dep_info);
    }

    deps
}

/// Detect how a single FK is used in the SELECT statement
fn detect_dependency_type(select_sql: &str, fk_col: &str) -> DependencyInfo {
    // Normalize SQL: remove extra whitespace, make lowercase for pattern matching
    let sql_normalized = select_sql
        .replace('\n', " ")
        .replace('\t', " ")
        .to_lowercase();

    // Infer view name from FK column
    // Convention: fk_user → v_user
    let view_name = if fk_col.starts_with("fk_") {
        format!("v_{}", &fk_col[3..])
    } else {
        return DependencyInfo::scalar(); // Can't infer view name
    };

    // Pattern 1: Nested Object
    // Look for: 'key_name', v_something.data
    // Example: 'author', v_user.data
    let nested_pattern = format!(r"'(\w+)',\s*{}.data", regex::escape(&view_name));
    if let Ok(re) = Regex::new(&nested_pattern) {
        if let Some(captures) = re.captures(&sql_normalized) {
            if let Some(key_match) = captures.get(1) {
                let key_name = key_match.as_str().to_string();
                return DependencyInfo::nested_object(key_name);
            }
        }
    }

    // Pattern 2: Array Aggregation
    // Look for: 'array_name', jsonb_agg(v_something.data ...)
    // Example: 'comments', jsonb_agg(v_comment.data ORDER BY ...)
    // Also handles COALESCE wrapper
    let array_pattern = format!(
        r"'(\w+)',\s*(?:coalesce\s*\()?\s*jsonb_agg\s*\(\s*{}.data",
        regex::escape(&view_name)
    );
    if let Ok(re) = Regex::new(&array_pattern) {
        if let Some(captures) = re.captures(&sql_normalized) {
            if let Some(key_match) = captures.get(1) {
                let array_name = key_match.as_str().to_string();
                // Convention: arrays use "id" as match key
                return DependencyInfo::array(array_name, "id".to_string());
            }
        }
    }

    // Default: Scalar (FK exists but not used in JSONB composition)
    DependencyInfo::scalar()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_nested_object_simple() {
        let sql = r#"
            SELECT pk_post, fk_user,
                   jsonb_build_object('id', id, 'author', v_user.data) AS data
            FROM tb_post
            LEFT JOIN v_user ON v_user.pk_user = fk_user
        "#;
        let fk_cols = vec!["fk_user".to_string()];

        let deps = analyze_dependencies(sql, &fk_cols);

        assert_eq!(deps.len(), 1, "Should detect 1 dependency");
        assert_eq!(deps[0].dep_type, DependencyType::NestedObject);
        assert_eq!(deps[0].jsonb_path, Some(vec!["author".to_string()]));
        assert_eq!(deps[0].array_match_key, None);
    }

    #[test]
    fn test_detect_array_simple() {
        let sql = r#"
            SELECT pk_user,
                   jsonb_build_object(
                       'id', id,
                       'posts', jsonb_agg(v_post.data ORDER BY created_at)
                   ) AS data
            FROM tb_user
            LEFT JOIN v_post ON v_post.fk_user = pk_user
            GROUP BY pk_user, id
        "#;
        let fk_cols = vec!["fk_post".to_string()]; // Inferred from v_post reference

        let deps = analyze_dependencies(sql, &fk_cols);

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].dep_type, DependencyType::Array);
        assert_eq!(deps[0].jsonb_path, Some(vec!["posts".to_string()]));
        assert_eq!(deps[0].array_match_key, Some("id".to_string())); // Convention
    }

    #[test]
    fn test_detect_scalar_direct_column() {
        let sql = r#"
            SELECT pk_post, jsonb_build_object('id', id, 'title', title) AS data
            FROM tb_post
        "#;
        let fk_cols = vec![]; // No FKs

        let deps = analyze_dependencies(sql, &fk_cols);

        assert_eq!(deps.len(), 0, "No dependencies for scalar-only TVIEW");
    }

    #[test]
    fn test_detect_multiple_dependencies() {
        let sql = r#"
            SELECT pk_post, fk_user, fk_category,
                   jsonb_build_object(
                       'id', id,
                       'title', title,
                       'author', v_user.data,
                       'category', v_category.data,
                       'comments', jsonb_agg(v_comment.data)
                   ) AS data
            FROM tb_post
            LEFT JOIN v_user ON v_user.pk_user = fk_user
            LEFT JOIN v_category ON v_category.pk_category = fk_category
            LEFT JOIN v_comment ON v_comment.fk_post = pk_post
            GROUP BY pk_post, fk_user, fk_category, v_user.data, v_category.data
        "#;
        let fk_cols = vec!["fk_user".to_string(), "fk_category".to_string(), "fk_comment".to_string()];

        let deps = analyze_dependencies(sql, &fk_cols);

        assert_eq!(deps.len(), 3);

        // fk_user → nested object
        assert_eq!(deps[0].dep_type, DependencyType::NestedObject);
        assert_eq!(deps[0].jsonb_path, Some(vec!["author".to_string()]));

        // fk_category → nested object
        assert_eq!(deps[1].dep_type, DependencyType::NestedObject);
        assert_eq!(deps[1].jsonb_path, Some(vec!["category".to_string()]));

        // fk_comment → array
        assert_eq!(deps[2].dep_type, DependencyType::Array);
        assert_eq!(deps[2].jsonb_path, Some(vec!["comments".to_string()]));
        assert_eq!(deps[2].array_match_key, Some("id".to_string()));
    }

    #[test]
    fn test_detect_no_fk_in_select() {
        // FK exists in schema but isn't referenced in SELECT
        let sql = r#"
            SELECT pk_post, jsonb_build_object('id', id) AS data
            FROM tb_post
        "#;
        let fk_cols = vec!["fk_user".to_string()];

        let deps = analyze_dependencies(sql, &fk_cols);

        // Should still return 1 dependency, but type = Scalar (not used)
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].dep_type, DependencyType::Scalar);
        assert_eq!(deps[0].jsonb_path, None);
    }
}

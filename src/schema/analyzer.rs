use crate::catalog::DependencyType;

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

/// Analyze SELECT statement to detect dependency types
///
/// # Arguments
/// * `select_sql` - The SELECT statement defining the TVIEW
/// * `fk_columns` - List of FK column names from schema inference
///
/// # Returns
/// Vector of DependencyInfo, one per FK column (order matches input)
pub fn analyze_dependencies(
    _select_sql: &str,
    _fk_columns: &[String],
) -> Vec<DependencyInfo> {
    // RED: Not implemented yet
    unimplemented!("Task 3 GREEN phase")
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

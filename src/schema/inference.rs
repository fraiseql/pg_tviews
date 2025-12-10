use super::{TViewSchema, parser};
use crate::error::TViewResult;

/// Infer PostgreSQL type for a column based on its SQL expression
///
/// This function analyzes the SQL expression to determine the appropriate
/// PostgreSQL type. For array columns, it detects ARRAY(...) subqueries
/// and infers element types.
pub fn infer_column_type(sql_expression: &str) -> String {
    let expr = sql_expression.trim();

    // Detect ARRAY(...) subqueries
    if expr.to_uppercase().starts_with("ARRAY(") {
        // For now, assume UUID arrays are common - could be enhanced
        // to analyze the subquery and infer element type
        return "UUID[]".to_string();
    }

    // Detect jsonb_agg (often used for arrays in JSONB)
    if expr.to_lowercase().contains("jsonb_agg(") {
        return "JSONB".to_string();
    }

    // Default to TEXT for other expressions
    "TEXT".to_string()
}

/// Infer TVIEW schema from SELECT statement
pub fn infer_schema(sql: &str) -> TViewResult<TViewSchema> {
    let columns_with_expressions = parser::parse_select_columns_with_expressions(sql)
        .map_err(|e| crate::error::TViewError::InvalidSelectStatement {
            sql: sql.to_string(),
            reason: e,
        })?;

    // Extract just column names for backward compatibility
    let columns: Vec<String> = columns_with_expressions.iter()
        .map(|(name, _)| name.clone())
        .collect();

    if columns.is_empty() {
        return Err(crate::error::TViewError::InvalidSelectStatement {
            sql: sql.to_string(),
            reason: "No columns found in SELECT statement".to_string(),
        });
    }

    let mut schema = TViewSchema::new();

    // 1. Detect pk_ column (highest priority - defines entity)
    for col in &columns {
        if let Some(entity) = col.strip_prefix("pk_") {
            schema.pk_column = Some(col.clone());
            schema.entity_name = Some(entity.to_string());
            break;
        }
    }

    // 2. Detect id column (Trinity identifier)
    if columns.contains(&"id".to_string()) {
        schema.id_column = Some("id".to_string());
    }

    // 3. Detect identifier column (optional Trinity identifier)
    if columns.contains(&"identifier".to_string()) {
        schema.identifier_column = Some("identifier".to_string());
    }

    // 4. Detect data column (JSONB read model)
    if columns.contains(&"data".to_string()) {
        schema.data_column = Some("data".to_string());
    }

    // 5. Detect fk_ columns (integer foreign keys for lineage)
    for col in &columns {
        if col.starts_with("fk_") {
            schema.fk_columns.push(col.clone());
        }
    }

    // 6. Detect _id columns (UUID foreign keys for filtering)
    // IMPORTANT: Exclude "id" itself (already handled above)
    for col in &columns {
        if col.ends_with("_id") && col != "id" {
            schema.uuid_fk_columns.push(col.clone());
        }
    }

    // 7. Additional columns with type inference (everything else)
    let reserved_columns: std::collections::HashSet<&str> = [
        schema.pk_column.as_deref().unwrap_or(""),
        schema.id_column.as_deref().unwrap_or(""),
        schema.identifier_column.as_deref().unwrap_or(""),
        schema.data_column.as_deref().unwrap_or(""),
    ].into_iter().filter(|s| !s.is_empty()).collect();

    for (col_name, col_expression) in &columns_with_expressions {
        if !reserved_columns.contains(col_name.as_str())
            && !schema.fk_columns.contains(col_name)
            && !schema.uuid_fk_columns.contains(col_name)
        {
            // Infer type for additional columns based on expression
            let inferred_type = infer_column_type(col_expression);
            schema.additional_columns.push(col_name.clone());
            schema.additional_columns_with_types.push((col_name.clone(), inferred_type));
        }
    }

    // Validate schema
    validate_schema(&schema)?;

    Ok(schema)
}

/// Validate inferred schema for required elements
fn validate_schema(schema: &TViewSchema) -> TViewResult<()> {
    // Warning: Missing pk_ column (not an error, but should warn)
    if schema.pk_column.is_none() {
        // In a real implementation, this would log a warning
        // pgrx::warning!("No pk_<entity> column found - lineage may not work correctly");
    }

    // Warning: Missing data column (not an error, but should warn)
    if schema.data_column.is_none() {
        // pgrx::warning!("No 'data' JSONB column found - read model may be incomplete");
    }

    // Error: Missing id column (required for Trinity identifier pattern)
    if schema.id_column.is_none() {
        return Err(crate::error::TViewError::RequiredColumnMissing {
            column_name: "id".to_string(),
            context: "Trinity identifier pattern requires 'id' column for external API".to_string(),
        });
    }

    // Error: Duplicate column names in different categories
    let mut all_categorized = std::collections::HashSet::new();

    if let Some(ref pk) = schema.pk_column {
        if !all_categorized.insert(pk) {
            return Err(crate::error::TViewError::InvalidSelectStatement {
                sql: "N/A".to_string(),
                reason: format!("Column '{}' appears in multiple categories", pk),
            });
        }
    }

    if let Some(ref id) = schema.id_column {
        if !all_categorized.insert(id) {
            return Err(crate::error::TViewError::InvalidSelectStatement {
                sql: "N/A".to_string(),
                reason: format!("Column '{}' appears in multiple categories", id),
            });
        }
    }

    if let Some(ref identifier) = schema.identifier_column {
        if !all_categorized.insert(identifier) {
            return Err(crate::error::TViewError::InvalidSelectStatement {
                sql: "N/A".to_string(),
                reason: format!("Column '{}' appears in multiple categories", identifier),
            });
        }
    }

    if let Some(ref data) = schema.data_column {
        if !all_categorized.insert(data) {
            return Err(crate::error::TViewError::InvalidSelectStatement {
                sql: "N/A".to_string(),
                reason: format!("Column '{}' appears in multiple categories", data),
            });
        }
    }

    for fk in &schema.fk_columns {
        if !all_categorized.insert(fk) {
            return Err(crate::error::TViewError::InvalidSelectStatement {
                sql: "N/A".to_string(),
                reason: format!("Column '{}' appears in multiple categories", fk),
            });
        }
    }

    for uuid_fk in &schema.uuid_fk_columns {
        if !all_categorized.insert(uuid_fk) {
            return Err(crate::error::TViewError::InvalidSelectStatement {
                sql: "N/A".to_string(),
                reason: format!("Column '{}' appears in multiple categories", uuid_fk),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_simple_schema() {
        let sql = "SELECT pk_post, id, data FROM tb_post";
        let schema = infer_schema(sql).unwrap();

        assert_eq!(schema.pk_column, Some("pk_post".to_string()));
        assert_eq!(schema.id_column, Some("id".to_string()));
        assert_eq!(schema.data_column, Some("data".to_string()));
        assert_eq!(schema.entity_name, Some("post".to_string()));
        assert!(schema.fk_columns.is_empty());
        assert!(schema.uuid_fk_columns.is_empty());
        assert!(schema.additional_columns.is_empty());
    }

    #[test]
    fn test_infer_complex_schema() {
        let sql = "SELECT pk_allocation, a.id, a.fk_machine, a.fk_location, m.id AS machine_id, l.id AS location_id, a.tenant_id, (a.start_date <= CURRENT_DATE) AS is_current, jsonb_build_object('id', a.id) AS data FROM tb_allocation a";
        let schema = infer_schema(sql).unwrap();

        assert_eq!(schema.pk_column, Some("pk_allocation".to_string()));
        assert_eq!(schema.id_column, Some("id".to_string()));
        assert_eq!(schema.data_column, Some("data".to_string()));
        assert_eq!(schema.entity_name, Some("allocation".to_string()));
        assert_eq!(schema.fk_columns, vec!["fk_machine", "fk_location"]);
        assert_eq!(schema.uuid_fk_columns, vec!["machine_id", "location_id", "tenant_id"]);
        assert_eq!(schema.additional_columns, vec!["is_current"]);
    }

    #[test]
    fn test_infer_missing_pk_column() {
        let sql = "SELECT id, name, data FROM tb_user";
        let schema = infer_schema(sql).unwrap();

        assert_eq!(schema.pk_column, None);
        assert_eq!(schema.id_column, Some("id".to_string()));
        assert_eq!(schema.data_column, Some("data".to_string()));
        assert_eq!(schema.entity_name, None);
    }

    #[test]
    fn test_infer_missing_data_column() {
        let sql = "SELECT pk_user, id, name FROM tb_user";
        let schema = infer_schema(sql).unwrap();

        assert_eq!(schema.pk_column, Some("pk_user".to_string()));
        assert_eq!(schema.id_column, Some("id".to_string()));
        assert_eq!(schema.data_column, None);
    }

    #[test]
    fn test_infer_missing_id_column_error() {
        let sql = "SELECT pk_user, name, data FROM tb_user";
        let result = infer_schema(sql);

        assert!(result.is_err());
        if let crate::error::TViewError::RequiredColumnMissing { column_name, .. } = result.unwrap_err() {
            assert_eq!(column_name, "id");
        } else {
            panic!("Expected RequiredColumnMissing error");
        }
    }

    #[test]
    fn test_infer_empty_select_error() {
        let sql = "SELECT FROM tb_user";
        let result = infer_schema(sql);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_schema_duplicate_columns() {
        let mut schema = TViewSchema::new();
        schema.pk_column = Some("id".to_string());
        schema.id_column = Some("id".to_string());

        let result = validate_schema(&schema);
        assert!(result.is_err());
    }

    #[test]
    fn test_infer_with_identifier_column() {
        let sql = "SELECT pk_product, id, identifier, name, data FROM tb_product";
        let schema = infer_schema(sql).unwrap();

        assert_eq!(schema.pk_column, Some("pk_product".to_string()));
        assert_eq!(schema.id_column, Some("id".to_string()));
        assert_eq!(schema.identifier_column, Some("identifier".to_string()));
        assert_eq!(schema.data_column, Some("data".to_string()));
        assert_eq!(schema.additional_columns, vec!["name"]);
    }
}
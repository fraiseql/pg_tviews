use pgrx::prelude::*;
use std::collections::HashMap;
use crate::error::TViewResult;

/// Infer column types from PostgreSQL catalog
pub fn infer_column_types(
    table_name: &str,
    columns: &[String],
) -> TViewResult<HashMap<String, String>> {
    let mut types = HashMap::new();

    for col in columns {
        // Query PostgreSQL catalog for column type
        let type_query = format!(
            "SELECT format_type(atttypid, atttypmod)
             FROM pg_attribute
             WHERE attrelid = '{}'::regclass
               AND attname = '{}'
               AND attnum > 0
               AND NOT attisdropped",
            table_name, col
        );

        let col_type = Spi::get_one::<String>(&type_query)
            .map_err(|e| crate::error::TViewError::SpiError {
                query: type_query.clone(),
                error: e.to_string(),
            })?
            .ok_or_else(|| crate::error::TViewError::CatalogError {
                operation: format!("find column '{col}' in table '{table_name}'"),
                pg_error: "Column not found".to_string(),
            })?;

        types.insert(col.clone(), col_type);
    }

    Ok(types)
}

/// Check if a table exists in the database
pub fn table_exists(table_name: &str) -> TViewResult<bool> {
    let query = format!(
        "SELECT COUNT(*) = 1 FROM pg_class
         WHERE relname = '{}' AND relkind = 'r'",
        table_name
    );

    Spi::get_one::<bool>(&query)
        .map_err(|e| crate::error::TViewError::SpiError {
            query,
            error: e.to_string(),
        })
        .map(|opt| opt.unwrap_or(false))
}

#[cfg(feature = "pg_test")]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use super::*;

    #[cfg(feature = "pg_test")]
    use pgrx_tests::pg_test;

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_infer_column_types() {
        // Create test table
        Spi::run("CREATE TABLE test_types (
            pk INTEGER PRIMARY KEY,
            id UUID NOT NULL,
            name TEXT,
            is_active BOOLEAN,
            created_at TIMESTAMPTZ DEFAULT NOW(),
            tags TEXT[],
            data JSONB
        )").unwrap();

        let columns = vec![
            "pk".to_string(),
            "id".to_string(),
            "name".to_string(),
            "is_active".to_string(),
            "created_at".to_string(),
            "tags".to_string(),
            "data".to_string(),
        ];

        let types = infer_column_types("test_types", &columns).unwrap();

        assert_eq!(types.get("pk"), Some(&"integer".to_string()));
        assert_eq!(types.get("id"), Some(&"uuid".to_string()));
        assert_eq!(types.get("name"), Some(&"text".to_string()));
        assert_eq!(types.get("is_active"), Some(&"boolean".to_string()));
        assert_eq!(types.get("created_at"), Some(&"timestamp with time zone".to_string()));
        assert_eq!(types.get("tags"), Some(&"text[]".to_string()));
        assert_eq!(types.get("data"), Some(&"jsonb".to_string()));
    }

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_infer_column_types_missing_table() {
        let columns = vec!["id".to_string()];
        let result = infer_column_types("nonexistent_table", &columns);
        assert!(result.is_err());
    }

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_infer_column_types_missing_column() {
        // Create test table
        Spi::run("CREATE TABLE test_missing_col (id UUID)").unwrap();

        let columns = vec!["id".to_string(), "missing_col".to_string()];
        let result = infer_column_types("test_missing_col", &columns);
        assert!(result.is_err());
    }

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_table_exists() {
        // Create test table
        Spi::run("CREATE TABLE test_exists (id UUID)").unwrap();

        assert_eq!(table_exists("test_exists"), Ok(true));
        assert_eq!(table_exists("nonexistent_table"), Ok(false));
    }
}
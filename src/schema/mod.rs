//! Schema Analysis: TVIEW Structure Inference and Validation
//!
//! This module analyzes SELECT statements to understand TVIEW structure:
//! - **Column Type Inference**: Determines `PostgreSQL` types for TVIEW columns
//! - **Primary Key Detection**: Identifies PK columns for refresh operations
//! - **Foreign Key Analysis**: Discovers relationships for cascade updates
//! - **Dependency Resolution**: Maps base tables to TVIEW columns
//!
//! ## Key Components
//!
//! - `TViewSchema`: Complete schema information for a TVIEW
//! - `infer_schema()`: Main entry point for schema analysis
//! - `parse_select_columns()`: Column extraction from SQL
//! - `infer_column_types()`: Type inference for columns
//!
//! ## Example
//!
//! ```rust
//! use pg_tviews::schema::inference::infer_schema;
//!
//! let sql = "SELECT id, name, data FROM users WHERE active = true";
//! let schema = infer_schema(sql)?;
//!
//! assert_eq!(schema.pk_column, Some("id".to_string()));
//! assert_eq!(schema.additional_columns, vec!["name".to_string(), "data".to_string()]);
//! ```

pub mod parser;
pub mod inference;
pub mod types;
pub mod analyzer;

use serde::{Serialize, Deserialize};
use pgrx::prelude::*;
use pgrx::JsonB;

/// Schema information inferred from a TVIEW SELECT statement
#[derive(Debug, Clone, Serialize, Deserialize, PostgresType, Default)]
pub struct TViewSchema {
    pub pk_column: Option<String>,
    pub id_column: Option<String>,
    pub identifier_column: Option<String>,
    pub data_column: Option<String>,
    pub fk_columns: Vec<String>,
    pub uuid_fk_columns: Vec<String>,
    pub additional_columns: Vec<String>,
    pub additional_columns_with_types: Vec<(String, String)>,
    pub entity_name: Option<String>,
}

impl TViewSchema {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn to_jsonb(&self) -> Result<JsonB, serde_json::Error> {
        let json_value = serde_json::to_value(self)?;
        Ok(JsonB(json_value))
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tview_schema_new() {
        let schema = TViewSchema::new();
        assert!(schema.pk_column.is_none());
        assert!(schema.id_column.is_none());
        assert!(schema.data_column.is_none());
        assert!(schema.entity_name.is_none());
        assert!(schema.fk_columns.is_empty());
        assert!(schema.uuid_fk_columns.is_empty());
        assert!(schema.additional_columns.is_empty());
    }

    #[test]
    fn test_tview_schema_serialization() {
        let mut schema = TViewSchema::new();
        schema.pk_column = Some("pk_post".to_string());
        schema.id_column = Some("id".to_string());
        schema.data_column = Some("data".to_string());
        schema.entity_name = Some("post".to_string());
        schema.fk_columns = vec!["fk_user".to_string()];
        schema.uuid_fk_columns = vec!["user_id".to_string()];

        let jsonb = schema.to_jsonb().unwrap();
        let json_value = jsonb.0;

        assert_eq!(json_value["pk_column"], "pk_post");
        assert_eq!(json_value["id_column"], "id");
        assert_eq!(json_value["data_column"], "data");
        assert_eq!(json_value["entity_name"], "post");
        assert_eq!(json_value["fk_columns"][0], "fk_user");
        assert_eq!(json_value["uuid_fk_columns"][0], "user_id");
    }
}
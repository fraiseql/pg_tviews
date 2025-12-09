pub mod parser;
pub mod inference;
pub mod types;

use serde::{Serialize, Deserialize};
use pgrx::prelude::*;
use pgrx::JsonB;

/// Schema information inferred from a TVIEW SELECT statement
#[derive(Debug, Clone, Serialize, Deserialize, PostgresType)]
#[serde(rename_all = "snake_case")]
pub struct TViewSchema {
    pub pk_column: Option<String>,
    pub id_column: Option<String>,
    pub identifier_column: Option<String>,
    pub data_column: Option<String>,
    pub fk_columns: Vec<String>,
    pub uuid_fk_columns: Vec<String>,
    pub additional_columns: Vec<String>,
    pub entity_name: Option<String>,
}

impl TViewSchema {
    pub fn new() -> Self {
        Self {
            pk_column: None,
            id_column: None,
            identifier_column: None,
            data_column: None,
            fk_columns: Vec::new(),
            uuid_fk_columns: Vec::new(),
            additional_columns: Vec::new(),
            entity_name: None,
        }
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
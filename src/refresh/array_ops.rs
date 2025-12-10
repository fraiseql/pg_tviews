//! Array Operations Module: INSERT/DELETE for JSONB Arrays
//!
//! This module provides functions to handle INSERT and DELETE operations
//! on JSONB array elements using jsonb_ivm functions. These operations
//! are triggered when source table rows are inserted or deleted.
//!
//! ## Architecture
//!
//! When a row is INSERTed into a source table:
//! 1. Detect if it contributes to an array in a parent TVIEW
//! 2. Use `jsonb_array_insert_where()` to add the element
//! 3. Maintain proper ordering (if specified)
//!
//! When a row is DELETEd from a source table:
//! 1. Find the element in the parent TVIEW array
//! 2. Use `jsonb_array_delete_where()` to remove it
//! 3. Preserve array integrity
//!
//! ## Performance
//!
//! - INSERT: O(n) where n = array length (find insertion point)
//! - DELETE: O(n) where n = array length (find element to remove)
//! - Both operations are surgical - only the affected array is modified

use pgrx::prelude::*;
use pgrx::JsonB;
use crate::error::{TViewError, TViewResult};

/// Insert an element into a JSONB array at the specified path
///
/// This function adds a new element to a JSONB array, maintaining proper
/// ordering if a sort key is specified.
///
/// # Arguments
/// * `table_name` - TVIEW table name (e.g., "tv_post")
/// * `pk_column` - Primary key column name (e.g., "pk_post")
/// * `pk_value` - Primary key value of the row to update
/// * `array_path` - JSONB path to the array (e.g., ["comments"])
/// * `new_element` - JSONB object to insert
/// * `sort_key` - Optional key for sorting (e.g., "created_at")
///
/// # Example
/// ```sql
/// -- Insert comment into post's comments array
/// SELECT insert_array_element(
///     'tv_post', 'pk_post', 1,
///     ARRAY['comments'], '{"id": "123", "text": "Hello"}'::jsonb,
///     'created_at'
/// );
/// ```
#[allow(dead_code)]
pub fn insert_array_element(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    array_path: &[String],
    new_element: JsonB,
    sort_key: Option<String>,
) -> TViewResult<()> {
    let path_str = array_path.join(",");
    let path_array = format!("ARRAY['{path_str}']");

    let sql = if let Some(key) = sort_key {
        // Insert with sorting
        format!(
            r"
            UPDATE {table_name} SET
                data = jsonb_array_insert_where(data, {path_array}, $1, '{key}', 'ASC'),
                updated_at = now()
            WHERE {pk_column} = $2
            "
        )
    } else {
        // Insert at end (no sorting)
        format!(
            r"
            UPDATE {table_name} SET
                data = jsonb_array_insert_where(data, {path_array}, $1, NULL, NULL),
                updated_at = now()
            WHERE {pk_column} = $2
            "
        )
    };

    Spi::run_with_args(
        &sql,
        Some(vec![
            (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), new_element.into_datum()),
            (PgOid::BuiltIn(PgBuiltInOids::INT8OID), pk_value.into_datum()),
        ]),
    ).map_err(|e| TViewError::SpiError {
        query: sql,
        error: e.to_string(),
    })?;

    Ok(())
}

/// Delete an element from a JSONB array at the specified path
///
/// This function removes an element from a JSONB array by matching
/// the specified key-value pair.
///
/// # Arguments
/// * `table_name` - TVIEW table name (e.g., "tv_post")
/// * `pk_column` - Primary key column name (e.g., "pk_post")
/// * `pk_value` - Primary key value of the row to update
/// * `array_path` - JSONB path to the array (e.g., ["comments"])
/// * `match_key` - Key to match for deletion (e.g., "id")
/// * `match_value` - Value to match for deletion
///
/// # Example
/// ```sql
/// -- Delete comment from post's comments array
/// SELECT delete_array_element(
///     'tv_post', 'pk_post', 1,
///     ARRAY['comments'], 'id', '"123"'::jsonb
/// );
/// ```
#[allow(dead_code)]
pub fn delete_array_element(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    array_path: &[String],
    match_key: &str,
    match_value: JsonB,
) -> TViewResult<()> {
    let path_str = array_path.join(",");
    let path_array = format!("ARRAY['{path_str}']");

    let sql = format!(
        r"
        UPDATE {table_name} SET
            data = jsonb_array_delete_where(data, {path_array}, '{match_key}', $1),
            updated_at = now()
        WHERE {pk_column} = $2
        "
    );

    Spi::run_with_args(
        &sql,
        Some(vec![
            (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), match_value.into_datum()),
            (PgOid::BuiltIn(PgBuiltInOids::INT8OID), pk_value.into_datum()),
        ]),
    ).map_err(|e| TViewError::SpiError {
        query: sql,
        error: e.to_string(),
    })?;

    Ok(())
}

/// Check if jsonb_ivm array functions are available
///
/// This is used to gracefully fall back if the extension isn't installed.
/// The array operations require jsonb_ivm for proper functionality.
#[allow(dead_code)]
pub fn check_array_functions_available() -> TViewResult<bool> {
    let sql = r"
        SELECT EXISTS(
            SELECT 1 FROM pg_proc
            WHERE proname = 'jsonb_array_insert_where'
               OR proname = 'jsonb_array_delete_where'
        )
    ";

    Spi::get_one::<bool>(sql)
        .map_err(|e| TViewError::SpiError {
            query: sql.to_string(),
            error: e.to_string(),
        })
        .map(|opt| opt.unwrap_or(false))
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use super::*;

    /// Test insert_array_element function
    #[pg_test]
    fn test_insert_array_element() {
        // Setup test table
        Spi::run(r#"
            CREATE TABLE test_array_ops (
                id BIGINT PRIMARY KEY,
                data JSONB DEFAULT '{"items": []}'::jsonb
            )
        "#).unwrap();

        Spi::run(r#"
            INSERT INTO test_array_ops VALUES (1, '{"items": []}'::jsonb)
        "#).unwrap();

        // Test insert
        let new_element = JsonB(serde_json::json!({"id": 1, "name": "Test Item"}));
        insert_array_element(
            "test_array_ops",
            "id",
            1,
            &["items".to_string()],
            new_element,
            None,
        ).unwrap();

        // Verify
        let result = Spi::get_one::<JsonB>("SELECT data FROM test_array_ops WHERE id = 1").unwrap().unwrap();
        let items = result.0["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "Test Item");
    }

    /// Test delete_array_element function
    #[pg_test]
    fn test_delete_array_element() {
        // Setup test table with array element
        Spi::run(r#"
            CREATE TABLE test_array_ops (
                id BIGINT PRIMARY KEY,
                data JSONB DEFAULT '{"items": [{"id": 1, "name": "Test Item"}]}'::jsonb
            )
        "#).unwrap();

        Spi::run(r#"
            INSERT INTO test_array_ops VALUES (1, '{"items": [{"id": 1, "name": "Test Item"}]}'::jsonb)
        "#).unwrap();

        // Test delete
        let match_value = JsonB(serde_json::json!(1));
        delete_array_element(
            "test_array_ops",
            "id",
            1,
            &["items".to_string()],
            "id",
            match_value,
        ).unwrap();

        // Verify
        let result = Spi::get_one::<JsonB>("SELECT data FROM test_array_ops WHERE id = 1").unwrap().unwrap();
        let items = result.0["items"].as_array().unwrap();
        assert_eq!(items.len(), 0);
    }

    /// Test array functions availability check
    #[pg_test]
    fn test_check_array_functions_available() {
        // This will depend on whether jsonb_ivm is installed
        let available = check_array_functions_available().unwrap();
        // We don't assert here since it depends on the test environment
        let _ = available;
    }
}
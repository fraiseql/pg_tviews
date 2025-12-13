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
use pgrx::datum::DatumWithOid;
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

    let args = vec![
        unsafe { DatumWithOid::new(new_element, PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
        unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) },
    ];
    Spi::run_with_args(
        &sql,
        &args,
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

    let args = vec![
        unsafe { DatumWithOid::new(match_value, PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
        unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) },
    ];
    Spi::run_with_args(
        &sql,
        &args,
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

/// Check if an array element with the given ID exists.
///
/// This function uses jsonb_ivm's optimized existence check when available,
/// providing ~10× performance improvement over jsonb_path_query.
///
/// **Security**: Validates all identifier parameters to prevent SQL injection.
///
/// # Arguments
///
/// * `data` - JSONB data containing the array
/// * `array_path` - Path to the array (e.g., ["comments"])
/// * `id_key` - Key to match (e.g., "id")
/// * `id_value` - Value to search for
///
/// # Returns
///
/// `true` if element exists, `false` otherwise
///
/// # Errors
///
/// Returns error if identifiers contain invalid characters or query fails.
///
/// # Performance
///
/// - With jsonb_ivm: ~10× faster than jsonb_path_query
/// - Without jsonb_ivm: Falls back to jsonb_path_query
///
/// # Example
///
/// ```rust
/// let data = JsonB(json!({
///     "comments": [
///         {"id": 1, "text": "Hello"},
///         {"id": 2, "text": "World"}
///     ]
/// }));
///
/// let exists = check_array_element_exists(
///     &data,
///     &["comments".to_string()],
///     "id",
///     &JsonB(json!(2))
/// )?;
/// assert!(exists);
/// ```
pub fn check_array_element_exists(
    data: &JsonB,
    array_path: &[String],
    id_key: &str,
    id_value: &JsonB,
) -> TViewResult<bool> {
    // Validate all identifiers to prevent SQL injection
    for segment in array_path {
        crate::validation::validate_sql_identifier(segment, "array_path_segment")?;
    }
    crate::validation::validate_sql_identifier(id_key, "id_key")?;
    crate::validation::validate_jsonb_path(&array_path.join("."), "array_path")?;

    // Check if jsonb_ivm is available
    let has_jsonb_ivm = check_array_functions_available()?;

    if has_jsonb_ivm {
        // Use optimized jsonb_ivm function
        // Now safe to use in format! after validation
        let path_str = array_path.join("','");
        let sql = format!(
            "SELECT jsonb_array_contains_id($1::jsonb, ARRAY['{}'], '{}', $2::jsonb)",
            path_str, id_key
        );

        Spi::get_one_with_args::<bool>(
            &sql,
            &[
                unsafe { DatumWithOid::new(JsonB(data.0.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
                unsafe { DatumWithOid::new(JsonB(id_value.0.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
            ][..],
        )
        .map_err(|e| TViewError::SpiError {
            query: sql,
            error: e.to_string(),
        })
        .map(|opt| opt.unwrap_or(false))
    } else {
        // Fallback to jsonb_path_query with correct syntax
        let path = array_path.join(".");
        // ✅ FIXED: Use [*] instead of **
        let sql = format!(
            "SELECT EXISTS(
                SELECT 1 FROM jsonb_path_query($1::jsonb, '$.{}[*] ? (@.{} == $2)')
            )",
            path, id_key
        );

        Spi::get_one_with_args::<bool>(
            &sql,
            &[
                unsafe { DatumWithOid::new(JsonB(data.0.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
                unsafe { DatumWithOid::new(JsonB(id_value.0.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
            ][..],
        )
        .map_err(|e| TViewError::SpiError {
            query: sql,
            error: e.to_string(),
        })
        .map(|opt| opt.unwrap_or(false))
    }
}

/// Insert array element only if it doesn't already exist.
///
/// This prevents duplicate entries in arrays by checking existence first
/// using the fast jsonb_array_contains_id() function.
///
/// # Arguments
///
/// Same as `insert_array_element()` plus:
/// * `id_key` - Key to check for duplicates (e.g., "id")
/// * `id_value` - ID value to check for duplicates
///
/// # Returns
///
/// - `Ok(true)` if element was inserted
/// - `Ok(false)` if element already exists (no insert)
/// - `Err` if operation failed
///
/// # Example
///
/// ```rust
/// // Will only insert if comment with id=123 doesn't exist
/// let inserted = insert_array_element_safe(
///     "tv_post",
///     "pk_post",
///     1,
///     &["comments".to_string()],
///     JsonB(json!({"id": 123, "text": "Hello"})),
///     None,
///     "id",
///     &JsonB(json!(123))
/// )?;
/// ```
#[allow(clippy::too_many_arguments)]
pub fn insert_array_element_safe(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    array_path: &[String],
    new_element: JsonB,
    sort_key: Option<String>,
    id_key: &str,
    id_value: &JsonB,
) -> TViewResult<bool> {
    // Validate all inputs to prevent SQL injection
    crate::validation::validate_table_name(table_name)?;
    crate::validation::validate_sql_identifier(pk_column, "pk_column")?;
    for segment in array_path {
        crate::validation::validate_sql_identifier(segment, "array_path_segment")?;
    }
    crate::validation::validate_sql_identifier(id_key, "id_key")?;
    crate::validation::validate_jsonb_path(&array_path.join("."), "array_path")?;
    if let Some(ref sort_key) = sort_key {
        crate::validation::validate_sql_identifier(sort_key, "sort_key")?;
    }

    // First, get current data
    let sql = format!("SELECT data FROM {} WHERE {} = $1", table_name, pk_column);
    let current_data = Spi::get_one_with_args::<JsonB>(
        &sql,
        &[unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) }][..],
    )
    .map_err(|e| TViewError::SpiError {
        query: sql.clone(),
        error: e.to_string(),
    })?;

    let current_data = match current_data {
        Some(data) => data,
        None => {
            return Err(TViewError::SpiError {
                query: sql,
                error: format!("No row found with {} = {}", pk_column, pk_value),
            });
        }
    };

    // Check if element already exists
    let exists = check_array_element_exists(&current_data, array_path, id_key, id_value)?;

    if exists {
        // Element already exists, skip insert
        info!(
            "Array element with {}={:?} already exists in {}.{}, skipping insert",
            id_key, id_value, table_name, array_path.join(".")
        );
        return Ok(false);
    }

    // First, get current data
    let sql = format!("SELECT data FROM {} WHERE {} = $1", table_name, pk_column);
    let current_data = Spi::get_one_with_args::<JsonB>(
        &sql,
        &[unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) }][..],
    )
    .map_err(|e| TViewError::SpiError {
        query: sql.clone(),
        error: e.to_string(),
    })?;

    let current_data = match current_data {
        Some(data) => data,
        None => {
            return Err(TViewError::SpiError {
                query: sql,
                error: format!("No row found with {} = {}", pk_column, pk_value),
            });
        }
    };

    // Check if element already exists
    let exists = check_array_element_exists(&current_data, array_path, id_key, id_value)?;

    if exists {
        // Element already exists, skip insert
        info!(
            "Array element with {}={:?} already exists in {}.{}, skipping insert",
            id_key, id_value, table_name, array_path.join(".")
        );
        return Ok(false);
    }

    // Element doesn't exist, perform insert
    insert_array_element(table_name, pk_column, pk_value, array_path, new_element, sort_key)?;
    Ok(true)
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use super::*;

    /// Test insert_array_element function
    #[cfg(any(test, feature = "pg_test"))]
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
    #[cfg(any(test, feature = "pg_test"))]
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
    #[cfg(any(test, feature = "pg_test"))]
    #[pg_test]
    fn test_check_array_functions_available() {
        // This will depend on whether jsonb_ivm is installed
        let available = check_array_functions_available().unwrap();
        // We don't assert here since it depends on the test environment
        let _ = available;
    }
}
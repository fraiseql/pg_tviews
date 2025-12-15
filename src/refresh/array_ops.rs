//! Array Operations Module: INSERT/DELETE for JSONB Arrays
//!
//! This module provides functions to handle INSERT and DELETE operations
//! on JSONB array elements using `jsonb_delta` functions. These operations
//! are triggered when source table rows are inserted or deleted.
//!
//! ## Architecture
//!
//! When a row is `INSERT`ed into a source table:
//! 1. Detect if it contributes to an array in a parent TVIEW
//! 2. Use `jsonb_array_insert_where()` to add the element
//! 3. Maintain proper ordering (if specified)
//!
//! When a row is `DELETE`d from a source table:
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
/// * `table_name` - TVIEW table name (e.g., `"tv_post"`)
/// * `pk_column` - Primary key column name (e.g., `"pk_post"`)
/// * `pk_value` - Primary key value of the row to update
/// * `array_path` - JSONB path to the array (e.g., ["comments"])
/// * `new_element` - JSONB object to insert
/// * `sort_key` - Optional key for sorting (e.g., `"created_at"`)
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
/// * `table_name` - TVIEW table name (e.g., `"tv_post"`)
/// * `pk_column` - Primary key column name (e.g., `"pk_post"`)
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

/// Check if `jsonb_delta` array functions are available
///
/// This is used to gracefully fall back if the extension isn't installed.
/// The array operations require `jsonb_delta` for proper functionality.
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
/// This function uses `jsonb_delta`'s optimized existence check when available,
/// providing ~10× performance improvement over `jsonb_path_query`.
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
/// - With `jsonb_delta`: ~10× faster than `jsonb_path_query`
/// - Without `jsonb_delta`: Falls back to `jsonb_path_query`
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
#[allow(dead_code)]  // Phase 1: Will be integrated in Phase 2+
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

    // Check if jsonb_delta is available
    let has_jsonb_delta = check_array_functions_available()?;

    if has_jsonb_delta {
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
/// using the fast `jsonb_array_contains_id()` function.
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
#[allow(dead_code)]  // Phase 1: Will be integrated in Phase 2+
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

/// Update a nested field within an array element using path notation.
///
/// This function surgically updates a nested field within a specific array element,
/// without replacing the entire element. Uses `jsonb_ivm`'s path-based update for
/// 2-3× performance improvement over full element replacement.
///
/// **Security**: Validates all identifier parameters to prevent SQL injection.
///
/// # Arguments
///
/// * `table_name` - TVIEW table name
/// * `pk_column` - Primary key column name
/// * `pk_value` - Primary key value
/// * `array_path` - Path to array (e.g., "items")
/// * `match_key` - Key to match elements (e.g., "id")
/// * `match_value` - Value to match for element selection
/// * `nested_path` - Dot-notation path within element (e.g., "author.name")
/// * `new_value` - New value to set at nested path
///
/// # Returns
///
/// `Ok(())` if update successful, `Err` if validation or update fails
///
/// # Errors
///
/// Returns error if identifiers contain invalid characters or query fails.
///
/// # Path Syntax
///
/// Nested paths support:
/// - Dot notation: `author.name` → object property access
/// - Array indexing: `tags[0]` → array element access
/// - Combined: `metadata.tags[0].value` → complex navigation
///
/// # Performance
///
/// - With `jsonb_ivm`: 2-3× faster than updating full element
/// - Without `jsonb_ivm`: Falls back to full element update
///
/// # Example
///
/// ```rust
/// // Update author name in a specific comment
/// update_array_element_path(
///     "tv_post",
///     "pk_post",
///     1,
///     "comments",
///     "id",
///     &JsonB(json!(123)),
///     "author.name",
///     &JsonB(json!("Alice Updated"))
/// )?;
///
/// // Before: {"comments": [{"id": 123, "author": {"name": "Alice", "email": "..."}, "text": "..."}]}
/// // After:  {"comments": [{"id": 123, "author": {"name": "Alice Updated", "email": "..."}, "text": "..."}]}
/// // Only author.name changed, rest of comment untouched
/// ```
#[allow(dead_code)]  // Phase 2: Will be integrated in Phase 3+
#[allow(clippy::too_many_arguments)]
pub fn update_array_element_path(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    array_path: &str,
    match_key: &str,
    match_value: &JsonB,
    nested_path: &str,
    new_value: &JsonB,
) -> TViewResult<()> {
    // Validate all inputs to prevent SQL injection
    crate::validation::validate_table_name(table_name)?;
    crate::validation::validate_sql_identifier(pk_column, "pk_column")?;
    crate::validation::validate_sql_identifier(match_key, "match_key")?;
    crate::validation::validate_jsonb_path(array_path, "array_path")?;
    crate::validation::validate_jsonb_path(nested_path, "nested_path")?;

    // Check if jsonb_ivm path function is available
    let has_jsonb_delta = check_path_function_available()?;

    if has_jsonb_delta {
        // Use optimized jsonb_delta path-based update
        let sql = format!(
            "UPDATE {} SET data = jsonb_delta_array_update_where_path(
                data, '{}', '{}', $1::jsonb, '{}', $2::jsonb
            ) WHERE {} = $3",
            table_name, array_path, match_key, nested_path, pk_column
        );

        Spi::run_with_args(&sql, &[
            unsafe { DatumWithOid::new(JsonB(match_value.0.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
            unsafe { DatumWithOid::new(JsonB(new_value.0.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
            unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) },
        ])?;
    } else {
        // Fallback: Use PostgreSQL's jsonb_set() for nested path update
        warning!(
            "jsonb_delta_array_update_where_path not available. \
             Using jsonb_set fallback (slower). \
             Install jsonb_delta >= 0.2.0 for 2-3× better performance."
        );

        // Build jsonb_set path: {array_path, [index], nested.path.parts}
        // First, find the array element index
        let find_index_sql = format!(
            "SELECT idx - 1 FROM {},
             jsonb_array_elements(data->'{}') WITH ORDINALITY arr(elem, idx)
             WHERE elem->>'{}' = $1::jsonb->>'{}' AND {} = $2
             LIMIT 1",
            table_name, array_path, match_key, match_key, pk_column
        );

        let element_index: Option<i32> = Spi::get_one_with_args(
            &find_index_sql,
            &[
                unsafe { DatumWithOid::new(JsonB(match_value.0.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
                unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) },
            ][..],
        )
        .map_err(|e| TViewError::SpiError {
            query: find_index_sql.clone(),
            error: e.to_string(),
        })?;

        let element_index = element_index.ok_or_else(|| TViewError::InvalidInput {
            parameter: "match_value".to_string(),
            value: format!("{}={:?}", match_key, match_value),
            reason: format!("No array element found with {}={:?}", match_key, match_value),
        })?;

        // Build jsonb_set path: {array_path, index, nested, path, parts}
        let nested_parts: Vec<&str> = nested_path.split('.').collect();
        let mut path_array = vec![array_path.to_string(), element_index.to_string()];
        path_array.extend(nested_parts.iter().map(|s| s.to_string()));

        let path_str = path_array.join(",");

        // Use jsonb_set to update nested field
        let update_sql = format!(
            "UPDATE {} SET data = jsonb_set(data, '{{{}}}'::text[], $1::jsonb) WHERE {} = $2",
            table_name, path_str, pk_column
        );

        Spi::run_with_args(&update_sql, &[
            unsafe { DatumWithOid::new(JsonB(new_value.0.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
            unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) },
        ])?;
    }

    Ok(())
}

/// Check if `jsonb_ivm` path functions are available
#[allow(dead_code)]  // Phase 2: Used by update_array_element_path
fn check_path_function_available() -> TViewResult<bool> {
    let result = Spi::get_one::<bool>(
        "SELECT EXISTS(SELECT 1 FROM pg_proc WHERE proname = 'jsonb_delta_array_update_where_path')"
    );
    match result {
        Ok(Some(exists)) => Ok(exists),
        _ => Ok(false), // Default to false if query fails
    }
}

#[cfg(feature = "pg_test")]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use super::*;

    /// Test insert_array_element function
    #[cfg(feature = "pg_test")]
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
    #[cfg(feature = "pg_test")]
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
    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_check_array_functions_available() {
        // This will depend on whether jsonb_ivm is installed
        let available = check_array_functions_available().unwrap();
        // We don't assert here since it depends on the test environment
        let _ = available;
    }
}
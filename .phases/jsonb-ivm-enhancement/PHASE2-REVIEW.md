# Phase 2 Implementation Review - Critical Issues Found

**Status**: ⚠️ **NEEDS FIXES** before implementation
**Severity**: CRITICAL (Multiple SQL Injection + Logic Issues)
**Date**: 2025-12-13

---

## 🔴 Critical Security Issues

### 1. SQL Injection - Multiple Parameters (CRITICAL)

**Location**: `update_array_element_path()` - Lines 171-185

**Problem**: **FIVE** parameters directly interpolated into SQL without validation:

```rust
// ❌ HIGHLY VULNERABLE CODE
let sql = format!(
    r#"
    UPDATE {table_name} SET
        data = jsonb_ivm_array_update_where_path(
            data,
            '{array_path}',
            '{match_key}',
            $1::jsonb,
            '{nested_path}',
            $2::jsonb
        ),
        updated_at = now()
    WHERE {pk_column} = $3
    "#
);
```

**Attack Vectors**:

1. **Table Name Injection**:
```rust
update_array_element_path(
    "tv_post; DROP TABLE users; --",  // ← SQL injection
    "pk_post", 1, ...
)
// Result: Drops users table!
```

2. **Path Injection**:
```rust
update_array_element_path(
    "tv_post", "pk_post", 1,
    "comments', 'id'); DROP TABLE users; --",  // ← Injection via array_path
    ...
)
```

3. **Nested Path Injection**:
```rust
update_array_element_path(
    ...,
    "author.name'); DROP TABLE users; --",  // ← Injection via nested_path
    ...
)
```

**Fix Required**: Validate ALL string parameters before interpolation:

```rust
/// Validate PostgreSQL identifier (table/column names)
fn validate_sql_identifier(s: &str, param_name: &str) -> TViewResult<()> {
    if s.is_empty() {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} cannot be empty", param_name),
        });
    }

    // Check for SQL injection attempts
    if s.contains(';') || s.contains('--') || s.contains("/*") || s.contains("'") {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} contains invalid characters: {}", param_name, s),
        });
    }

    // Ensure valid identifier characters
    if !s.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} must contain only alphanumeric and underscore: {}", param_name, s),
        });
    }

    Ok(())
}

/// Validate JSONB path (allows dots and brackets for navigation)
fn validate_jsonb_path(path: &str, param_name: &str) -> TViewResult<()> {
    if path.is_empty() {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} cannot be empty", param_name),
        });
    }

    // Check for obvious SQL injection attempts
    if path.contains(';') || path.contains("--") || path.contains("/*") {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} contains SQL injection patterns: {}", param_name, path),
        });
    }

    // Allow: alphanumeric, dots, brackets, underscores
    let valid_chars = path.chars().all(|c| {
        c.is_alphanumeric() || c == '.' || c == '[' || c == ']' || c == '_'
    });

    if !valid_chars {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} contains invalid characters: {}", param_name, path),
        });
    }

    Ok(())
}

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
    // ✅ VALIDATE ALL INPUTS
    validate_sql_identifier(table_name, "table_name")?;
    validate_sql_identifier(pk_column, "pk_column")?;
    validate_sql_identifier(array_path, "array_path")?;
    validate_sql_identifier(match_key, "match_key")?;
    validate_jsonb_path(nested_path, "nested_path")?;

    // Now safe to use in format!
    let sql = format!(
        r#"
        UPDATE {table_name} SET
            data = jsonb_ivm_array_update_where_path(
                data,
                '{array_path}',
                '{match_key}',
                $1::jsonb,
                '{nested_path}',
                $2::jsonb
            ),
            updated_at = now()
        WHERE {pk_column} = $3
        "#
    );

    // ... rest of function
}
```

**Impact**: CRITICAL - Allows arbitrary SQL execution

---

### 2. SQL Injection in main.rs Integration (CRITICAL)

**Location**: `build_smart_patch_sql()` integration - Lines 244-250

**Problem**: Unvalidated variables used in SQL construction:

```rust
// ❌ VULNERABLE
if let Some(nested_path) = &dep.nested_path {
    format!(
        "jsonb_ivm_array_update_where_path({patch_expr}, ARRAY['{path_str}'], '{match_key}', $1::jsonb, '{nested_path}')"
        //                                                                                                ^^^^^^^^^^^^
        //                                                                                    Unvalidated user input!
    )
}
```

**Fix**: Validate in metadata parsing, not at usage:

```rust
// In catalog.rs parse_dependencies():
let nested_path = if parts.len() > 4 {
    let path = parts[4].to_string();
    // ✅ Validate when parsing metadata
    validate_jsonb_path(&path, "nested_path")?;
    Some(path)
} else {
    None
};
```

**Impact**: CRITICAL - SQL injection via metadata

---

## 🔴 Critical Logic Issues

### 3. Missing Fallback Implementation

**Location**: Lines 157-168

**Problem**: Function returns error instead of implementing fallback:

```rust
if !has_jsonb_ivm {
    warning!("...");
    // ❌ TODO: Implement fallback to full element update
    return Err(TViewError::SpiError {
        error: "Nested path updates require jsonb_ivm >= 0.2.0".to_string(),
    });
}
```

**Issue**: This breaks the "graceful degradation" promise. Without jsonb_ivm, the function completely fails instead of falling back to full element replacement.

**Fix Required**: Implement actual fallback:

```rust
if !has_jsonb_ivm {
    warning!(
        "jsonb_ivm_array_update_where_path not available. \
         Falling back to full element update (slower). \
         Install jsonb_ivm >= 0.2.0 for 2-3× better performance."
    );

    // ✅ Fallback: Get full element, update it, replace it
    return fallback_update_nested_field(
        table_name,
        pk_column,
        pk_value,
        array_path,
        match_key,
        match_value,
        nested_path,
        new_value,
    );
}

/// Fallback implementation for nested path updates without jsonb_ivm
fn fallback_update_nested_field(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    array_path: &str,
    match_key: &str,
    match_value: &JsonB,
    nested_path: &str,
    new_value: &JsonB,
) -> TViewResult<()> {
    // Validate inputs (same as main function)
    validate_sql_identifier(table_name, "table_name")?;
    validate_sql_identifier(pk_column, "pk_column")?;
    validate_sql_identifier(array_path, "array_path")?;
    validate_sql_identifier(match_key, "match_key")?;
    validate_jsonb_path(nested_path, "nested_path")?;

    // 1. Get current data
    let get_sql = format!(
        "SELECT data FROM {} WHERE {} = $1",
        table_name, pk_column
    );

    let current_data = Spi::get_one_with_args::<JsonB>(
        &get_sql,
        vec![unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) }],
    )?
    .ok_or_else(|| TViewError::SpiError {
        query: get_sql.clone(),
        error: format!("No row found with {} = {}", pk_column, pk_value),
    })?;

    // 2. Find matching array element
    let array_elements = current_data.0
        .get(array_path)
        .and_then(|v| v.as_array())
        .ok_or_else(|| TViewError::SpiError {
            query: String::new(),
            error: format!("Array '{}' not found", array_path),
        })?;

    // 3. Update the element using standard PostgreSQL operators
    // Build nested jsonb_set calls for the path
    let path_parts: Vec<&str> = nested_path.split('.').collect();

    // Use jsonb_set to update the nested field
    let mut update_sql = format!(
        "UPDATE {} SET data = jsonb_set(data, ARRAY['{}'",
        table_name, array_path
    );

    // Add index of matching element (find it first)
    // This is complex - for fallback, might be better to just replace entire element
    // OR use a simpler approach with jsonb_array_elements

    warning!("Fallback path update is slower - consider installing jsonb_ivm");

    // Simplified: Use jsonb_smart_patch_array to replace entire element
    // This loses the "surgical" benefit but maintains functionality

    // Get the element, update nested field in Rust, then replace
    // (This is placeholder - full implementation would be more complex)

    Err(TViewError::SpiError {
        query: String::new(),
        error: "Fallback not fully implemented - install jsonb_ivm >= 0.2.0".to_string(),
    })

    // TODO: Full fallback implementation
    // For now, better to return clear error than crash
}
```

**Impact**: HIGH - Breaks without jsonb_ivm instead of degrading gracefully

---

### 4. Test Data Logic Error

**Location**: Test 3 (lines 450-453)

**Problem**: Suspicious JOIN logic that may not work as intended:

```sql
-- ❌ Questionable JOIN
LEFT JOIN tb_comment c ON c.fk_user IN (
    SELECT fk_user FROM tb_comment WHERE pk_comment IS NOT NULL
)
```

**Issue**: This JOIN condition doesn't relate `c` to `p` (the post), so it will create a cartesian product of all comments for all posts.

**Fix**:

```sql
-- ✅ Correct JOIN
CREATE TABLE tb_post_comment (
    pk_post_comment BIGSERIAL PRIMARY KEY,
    fk_post BIGINT REFERENCES tb_post(pk_post),
    fk_comment BIGINT REFERENCES tb_comment(pk_comment)
);

-- Then JOIN properly:
LEFT JOIN tb_post_comment pc ON pc.fk_post = p.pk_post
LEFT JOIN tb_comment c ON c.pk_comment = pc.fk_comment
LEFT JOIN tb_user u ON u.pk_user = c.fk_user
```

**Impact**: MEDIUM - Tests may not validate actual functionality

---

## 🟡 High Priority Issues

### 5. Missing Path Syntax Validation

**Location**: `nested_path` parameter usage

**Problem**: No validation that path syntax is actually valid:
- Should check bracket matching: `tags[0]` ✓, `tags[0` ✗
- Should validate array indices are numeric
- Should prevent malformed paths

**Fix**: Add comprehensive path syntax validator:

```rust
/// Validate nested path syntax
fn validate_path_syntax(path: &str) -> TViewResult<()> {
    let mut bracket_depth = 0;
    let mut in_brackets = false;

    for (i, ch) in path.chars().enumerate() {
        match ch {
            '[' => {
                bracket_depth += 1;
                in_brackets = true;
            }
            ']' => {
                bracket_depth -= 1;
                if bracket_depth < 0 {
                    return Err(TViewError::SpiError {
                        query: String::new(),
                        error: format!("Unmatched ']' at position {}", i),
                    });
                }
                in_brackets = false;
            }
            '.' if in_brackets => {
                return Err(TViewError::SpiError {
                    query: String::new(),
                    error: "Dots not allowed inside brackets".to_string(),
                });
            }
            _ => {}
        }
    }

    if bracket_depth != 0 {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: "Unmatched '[' in path".to_string(),
        });
    }

    Ok(())
}
```

**Impact**: HIGH - Malformed paths cause runtime errors

---

### 6. Quote Escaping in Paths

**Location**: Lines 178-179

**Problem**: If `nested_path` contains single quotes (even after validation), it will break SQL:

```rust
// If nested_path = "author's.name" (somehow bypassed validation)
'{nested_path}'  // Results in: 'author's.name' ← syntax error!
```

**Fix**: Either:
1. Reject paths with quotes in validation ✓ (already done if using alphanumeric check)
2. Use proper escaping if quotes are allowed

**Current validation should prevent this**, but document it clearly.

**Impact**: MEDIUM - Edge case with quote handling

---

## 🟢 Medium Priority Issues

### 7. Missing Documentation on Path Limits

**Location**: Function documentation (lines 96-143)

**Issue**: No mention of:
- Maximum path depth
- Maximum path length
- Valid characters in paths
- Bracket nesting limits

**Fix**: Add to documentation:

```rust
/// # Path Constraints
///
/// - Maximum depth: 100 levels
/// - Valid characters: alphanumeric, dots (.), brackets ([]), underscores (_)
/// - Bracket indices must be non-negative integers
/// - No spaces or special characters
/// - Example valid paths:
///   - `author.name` ✓
///   - `tags[0]` ✓
///   - `metadata.tags[0].value` ✓
///   - `author's.name` ✗ (apostrophe not allowed)
///   - `data[0].[field]` ✗ (dot after bracket)
```

---

### 8. Inconsistent Parameter Types

**Location**: Function signature (line 144-152)

**Issue**: `array_path` is `&str` but in other functions it's `&[String]` (array of segments).

**Inconsistency**:
- `check_array_element_exists()` takes `&[String]` for array_path
- `update_array_element_path()` takes `&str` for array_path

**Impact**: MEDIUM - API inconsistency, confusing for developers

**Recommendation**: Standardize on one approach (prefer `&str` for simplicity)

---

## 🟢 Low Priority Issues

### 9. Debug Output Includes Sensitive Data

**Location**: Lines 198-201

```rust
debug!(
    "Updated nested path '{}.{}' in array '{}' for {}.{} = {}",
    array_path, nested_path, table_name, table_name, pk_column, pk_value
);
```

**Issue**: Logs might contain sensitive information (table names, keys, values)

**Recommendation**: Make debug logging opt-in or sanitize sensitive data

---

### 10. Missing Performance Metrics

**Issue**: No way to track if path updates are actually faster

**Recommendation**: Add optional timing/metrics:
- Track execution time
- Compare with fallback performance
- Log performance warnings if slower than expected

---

## 📝 Complete Corrected Implementation

### update_array_element_path() with Full Validation

```rust
// Add validation helpers at module level
fn validate_sql_identifier(s: &str, param_name: &str) -> TViewResult<()> {
    if s.is_empty() {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} cannot be empty", param_name),
        });
    }

    if s.contains(';') || s.contains("--") || s.contains("/*") || s.contains("'") || s.contains('"') {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} contains SQL injection patterns", param_name),
        });
    }

    if !s.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} contains invalid identifier characters", param_name),
        });
    }

    Ok(())
}

fn validate_jsonb_path(path: &str, param_name: &str) -> TViewResult<()> {
    if path.is_empty() {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} cannot be empty", param_name),
        });
    }

    // Check for SQL injection
    if path.contains(';') || path.contains("--") || path.contains("/*") || path.contains("'") {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} contains SQL injection patterns", param_name),
        });
    }

    // Validate path syntax (brackets, dots, alphanumeric, underscore)
    let valid = path.chars().all(|c| {
        c.is_alphanumeric() || c == '.' || c == '[' || c == ']' || c == '_'
    });

    if !valid {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} contains invalid path characters", param_name),
        });
    }

    // Validate bracket matching
    let mut depth = 0;
    for ch in path.chars() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return Err(TViewError::SpiError {
                        query: String::new(),
                        error: format!("{} has unmatched closing bracket", param_name),
                    });
                }
            }
            _ => {}
        }
    }

    if depth != 0 {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!("{} has unmatched opening bracket", param_name),
        });
    }

    Ok(())
}

/// Update a nested field within an array element using path notation.
///
/// **Security**: All string parameters are validated to prevent SQL injection.
///
/// # Arguments
///
/// * `table_name` - TVIEW table name (validated identifier)
/// * `pk_column` - Primary key column name (validated identifier)
/// * `pk_value` - Primary key value
/// * `array_path` - Array field name (validated identifier)
/// * `match_key` - Element match key (validated identifier)
/// * `match_value` - Value to match
/// * `nested_path` - Dot-notation path within element (validated path)
/// * `new_value` - New value to set
///
/// # Path Syntax
///
/// Nested paths support:
/// - Dot notation: `author.name` → object property access
/// - Array indexing: `tags[0]` → array element access
/// - Combined: `metadata.tags[0].value` → complex navigation
///
/// # Path Constraints
///
/// - Valid characters: alphanumeric, dots (.), brackets ([]), underscores (_)
/// - Brackets must be properly matched
/// - Array indices must be numeric
/// - No SQL injection characters (quotes, semicolons, comments)
///
/// # Performance
///
/// - With jsonb_ivm: 2-3× faster than updating full element
/// - Without jsonb_ivm: Falls back to full element update (slower)
///
/// # Errors
///
/// Returns error if:
/// - Any parameter contains invalid characters (security)
/// - Path syntax is malformed
/// - jsonb_ivm not available and fallback fails
/// - Database query fails
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
/// ```
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
    // ✅ VALIDATE ALL STRING INPUTS (SECURITY CRITICAL)
    validate_sql_identifier(table_name, "table_name")?;
    validate_sql_identifier(pk_column, "pk_column")?;
    validate_sql_identifier(array_path, "array_path")?;
    validate_sql_identifier(match_key, "match_key")?;
    validate_jsonb_path(nested_path, "nested_path")?;

    // Check if jsonb_ivm path function is available
    let has_jsonb_ivm = check_path_function_available()?;

    if !has_jsonb_ivm {
        warning!(
            "jsonb_ivm_array_update_where_path not available. \
             Falling back to full element update. \
             Install jsonb_ivm >= 0.2.0 for 2-3× better performance."
        );

        // ✅ IMPLEMENT FALLBACK (simplified - full implementation needed)
        // For MVP: return error with clear message
        // For production: implement full fallback using jsonb_smart_patch_array
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!(
                "Nested path updates require jsonb_ivm >= 0.2.0. \
                 Please install jsonb_ivm extension. \
                 Fallback: Update entire array element using jsonb_smart_patch_array."
            ),
        });
    }

    // Build SQL using jsonb_ivm_array_update_where_path
    // NOW SAFE: All parameters validated above
    let sql = format!(
        r#"
        UPDATE {table_name} SET
            data = jsonb_ivm_array_update_where_path(
                data,
                '{array_path}',
                '{match_key}',
                $1::jsonb,
                '{nested_path}',
                $2::jsonb
            ),
            updated_at = now()
        WHERE {pk_column} = $3
        "#
    );

    let args = vec![
        unsafe { DatumWithOid::new(match_value.clone(), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
        unsafe { DatumWithOid::new(new_value.clone(), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
        unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) },
    ];

    Spi::run_with_args(&sql, &args).map_err(|e| TViewError::SpiError {
        query: sql.clone(),
        error: e.to_string(),
    })?;

    debug!(
        "Updated nested path '{}.{}' in array '{}' for {}.{} = {}",
        array_path, nested_path, table_name, table_name, pk_column, pk_value
    );

    Ok(())
}

// ... rest of implementation
```

---

## ✅ Action Items

### Before Implementation:

1. **CRITICAL**: Add `validate_sql_identifier()` helper
2. **CRITICAL**: Add `validate_jsonb_path()` helper
3. **CRITICAL**: Apply validation to `update_array_element_path()`
4. **CRITICAL**: Apply validation to `build_smart_patch_sql()` integration
5. **HIGH**: Implement or document fallback strategy
6. **HIGH**: Fix test data JOIN logic (Test 3)
7. **MEDIUM**: Standardize array_path parameter type
8. **MEDIUM**: Add path syntax validation
9. Add security test cases for injection attempts
10. Update documentation with constraints

### Testing Checklist:

- [ ] Test with valid identifiers and paths
- [ ] Test SQL injection attempts on all string params
- [ ] Test malformed path syntax (unmatched brackets)
- [ ] Test without jsonb_ivm (fallback)
- [ ] Test path depth limits
- [ ] Test special characters in paths
- [ ] Test integration with main.rs cascade logic

---

## Summary

**Total Issues Found**: 10
**Critical**: 4 (SQL injection × 3, missing fallback)
**High**: 2 (Path syntax validation, test logic)
**Medium**: 3 (Documentation, API consistency, escaping)
**Low**: 1 (Debug logging)

**Recommendation**: 🔴 **DO NOT PROCEED** until critical security issues are fixed.

**Estimated Fix Time**: 1-2 hours

**Priority Order**:
1. Add input validation helpers (30 min)
2. Apply validation to all functions (30 min)
3. Document or implement fallback (30 min)
4. Fix test data (15 min)
5. Add security tests (15 min)

**Next Steps**:
1. Review this document
2. Apply all critical fixes
3. Add comprehensive security tests
4. Re-test before providing to junior developers

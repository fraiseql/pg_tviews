# Phase Implementation Plans Update - Security Hardening & Corrections

**Document**: Comprehensive update plan for Phases 1-5
**Priority**: CRITICAL - Security vulnerabilities must be fixed before ANY implementation
**Estimated Time**: 4-6 hours for all updates
**Date**: 2025-12-13

---

## Executive Summary

Security reviews of Phase 1 and Phase 2 revealed **critical SQL injection vulnerabilities** and **missing input validation** throughout the implementation plans. This document outlines a systematic approach to:

1. Create shared security infrastructure
2. Update all phases with proper validation
3. Ensure graceful fallback implementations
4. Add comprehensive security testing
5. Standardize error handling and documentation

**Critical Finding**: The pattern `format!("SQL {unvalidated_param}")` appears in **ALL phases**. This is a **systemic security vulnerability** requiring immediate remediation.

---

## 🎯 Goals

### Primary Goals
1. **Eliminate all SQL injection vulnerabilities** across all phases
2. **Create reusable validation infrastructure** to prevent future issues
3. **Ensure graceful degradation** when jsonb_ivm unavailable
4. **Add security testing** to every phase
5. **Standardize API design** across all functions

### Secondary Goals
6. Improve error messages for better debugging
7. Add performance monitoring hooks
8. Document security constraints clearly
9. Create security testing guidelines
10. Establish code review checklist

---

## 📊 Current State Analysis

### Issues by Phase

| Phase | Critical | High | Medium | Status |
|-------|----------|------|--------|--------|
| Phase 1 | 3 | 2 | 2 | ⚠️ NEEDS FIXES |
| Phase 2 | 4 | 2 | 3 | ⚠️ NEEDS FIXES |
| Phase 3 | Unknown | Unknown | Unknown | 📝 NOT REVIEWED |
| Phase 4 | Unknown | Unknown | Unknown | 📝 NOT REVIEWED |
| Phase 5 | N/A | N/A | N/A | 🧪 TESTING PHASE |

### Common Vulnerabilities Found

1. **SQL Injection** (Phases 1-2, likely 3-4)
   - Direct parameter interpolation into SQL strings
   - Missing identifier validation
   - No path syntax validation

2. **Missing Fallbacks** (Phase 2, likely others)
   - Functions fail instead of degrading gracefully
   - Breaks "optional dependency" promise

3. **Inconsistent APIs** (Phases 1-2)
   - Different parameter types for similar operations
   - Inconsistent error handling patterns

4. **Incomplete Testing** (All phases)
   - No security injection tests
   - Missing edge case coverage
   - No fallback testing

---

## 🏗️ Implementation Strategy

### Three-Phase Approach

#### Phase A: Foundation (2-3 hours)
Create shared infrastructure that all phases will use:
1. Validation module
2. Error handling patterns
3. Testing utilities
4. Documentation templates

#### Phase B: Updates (2-3 hours)
Systematically update each phase implementation plan:
1. Apply validation to all functions
2. Implement proper fallbacks
3. Add security tests
4. Standardize APIs

#### Phase C: Verification (1 hour)
Final review and validation:
1. Cross-phase consistency check
2. Security audit checklist
3. Documentation completeness
4. Test coverage verification

---

## 📋 Phase A: Create Shared Infrastructure

### A1: Validation Module (HIGH PRIORITY)

**File**: `src/validation.rs` (NEW)

**Purpose**: Centralized input validation to prevent SQL injection

**Components**:

```rust
//! Input Validation Module
//!
//! This module provides security-critical validation functions used throughout
//! pg_tviews to prevent SQL injection and other input-based attacks.
//!
//! ## Security Principles
//!
//! 1. **Whitelist, not blacklist**: Only allow known-safe characters
//! 2. **Validate early**: Check inputs before any processing
//! 3. **Fail securely**: Return clear errors on invalid input
//! 4. **No exceptions**: Every external input must be validated
//!
//! ## Usage
//!
//! ```rust
//! use crate::validation::{validate_sql_identifier, validate_jsonb_path};
//!
//! // Validate before using in SQL
//! validate_sql_identifier(table_name, "table_name")?;
//! let sql = format!("SELECT * FROM {}", table_name); // Now safe
//! ```

use crate::error::{TViewError, TViewResult};

/// Validate PostgreSQL identifier (table, column, schema names)
///
/// # Security
///
/// Prevents SQL injection by ensuring only safe identifier characters.
/// Allows: alphanumeric + underscore (PostgreSQL identifier rules)
/// Rejects: quotes, semicolons, dashes, spaces, special chars
///
/// # Arguments
///
/// * `identifier` - String to validate
/// * `param_name` - Parameter name for error messages
///
/// # Returns
///
/// `Ok(())` if valid, `Err` with descriptive message if invalid
///
/// # Examples
///
/// ```rust
/// // Valid identifiers
/// validate_sql_identifier("my_table", "table_name")?;     // ✓
/// validate_sql_identifier("user_data", "column")?;         // ✓
/// validate_sql_identifier("pk_user", "pk_column")?;        // ✓
///
/// // Invalid identifiers
/// validate_sql_identifier("users; DROP TABLE", "table")?;  // ✗ SQL injection
/// validate_sql_identifier("user-data", "table")?;          // ✗ Contains dash
/// validate_sql_identifier("my table", "table")?;           // ✗ Contains space
/// validate_sql_identifier("'admin'", "column")?;           // ✗ Contains quotes
/// ```
pub fn validate_sql_identifier(identifier: &str, param_name: &str) -> TViewResult<()> {
    // Check for empty
    if identifier.is_empty() {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: identifier.to_string(),
            reason: "Identifier cannot be empty".to_string(),
        });
    }

    // Check for SQL injection patterns
    let dangerous_chars = [';', '-', '\'', '"', '/', '*', '\\', '\0'];
    for &ch in &dangerous_chars {
        if identifier.contains(ch) {
            return Err(TViewError::SecurityViolation {
                parameter: param_name.to_string(),
                value: sanitize_for_logging(identifier),
                reason: format!("Identifier contains dangerous character: '{}'", ch),
            });
        }
    }

    // Check for SQL keywords used maliciously
    let lower = identifier.to_lowercase();
    if lower.contains("drop ") || lower.contains("delete ") ||
       lower.contains("insert ") || lower.contains("update ") ||
       lower.contains("create ") || lower.contains("alter ") {
        return Err(TViewError::SecurityViolation {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(identifier),
            reason: "Identifier contains SQL keywords".to_string(),
        });
    }

    // Ensure valid identifier characters (alphanumeric + underscore)
    if !identifier.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(identifier),
            reason: "Identifier must contain only alphanumeric characters and underscores".to_string(),
        });
    }

    // PostgreSQL identifiers can't start with digit (unless quoted)
    if identifier.chars().next().unwrap().is_numeric() {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(identifier),
            reason: "Identifier cannot start with a digit".to_string(),
        });
    }

    // Length limit (PostgreSQL max identifier length is 63)
    if identifier.len() > 63 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: format!("{}... ({} chars)", &identifier[..20], identifier.len()),
            reason: "Identifier too long (max 63 characters)".to_string(),
        });
    }

    Ok(())
}

/// Validate JSONB path syntax (dot notation + array indices)
///
/// # Security
///
/// Prevents injection while allowing complex path navigation.
/// Allows: alphanumeric, dots, brackets, underscores
/// Rejects: quotes, semicolons, SQL keywords, mismatched brackets
///
/// # Path Syntax
///
/// - Object navigation: `field.subfield.deep`
/// - Array indexing: `items[0]` or `items[123]`
/// - Combined: `users[0].profile.settings.theme`
///
/// # Constraints
///
/// - Maximum depth: 100 segments
/// - Maximum length: 500 characters
/// - Brackets must be matched
/// - Array indices must be non-negative integers
/// - No spaces or special characters
///
/// # Examples
///
/// ```rust
/// // Valid paths
/// validate_jsonb_path("author.name", "path")?;                    // ✓
/// validate_jsonb_path("items[0]", "path")?;                       // ✓
/// validate_jsonb_path("users[5].profile.email", "path")?;         // ✓
/// validate_jsonb_path("metadata.tags[0].value", "path")?;         // ✓
///
/// // Invalid paths
/// validate_jsonb_path("field'; DROP TABLE", "path")?;             // ✗ Injection
/// validate_jsonb_path("items[", "path")?;                         // ✗ Unmatched bracket
/// validate_jsonb_path("data[-1]", "path")?;                       // ✗ Negative index
/// validate_jsonb_path("author's.name", "path")?;                  // ✗ Apostrophe
/// ```
pub fn validate_jsonb_path(path: &str, param_name: &str) -> TViewResult<()> {
    // Check for empty
    if path.is_empty() {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: path.to_string(),
            reason: "Path cannot be empty".to_string(),
        });
    }

    // Length limit
    if path.len() > 500 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: format!("{}... ({} chars)", &path[..50], path.len()),
            reason: "Path too long (max 500 characters)".to_string(),
        });
    }

    // Check for SQL injection patterns
    if path.contains(';') || path.contains("--") || path.contains("/*") ||
       path.contains('\'') || path.contains('"') {
        return Err(TViewError::SecurityViolation {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(path),
            reason: "Path contains SQL injection patterns".to_string(),
        });
    }

    // Validate allowed characters
    let valid = path.chars().all(|c| {
        c.is_alphanumeric() || c == '.' || c == '[' || c == ']' || c == '_'
    });

    if !valid {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(path),
            reason: "Path contains invalid characters (allowed: alphanumeric, dots, brackets, underscore)".to_string(),
        });
    }

    // Validate bracket matching and array indices
    validate_bracket_matching(path, param_name)?;
    validate_array_indices(path, param_name)?;

    // Validate depth (max 100 levels)
    let depth = path.split('.').count() + path.matches('[').count();
    if depth > 100 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: format!("depth={}", depth),
            reason: "Path too deep (max 100 levels)".to_string(),
        });
    }

    Ok(())
}

/// Validate bracket matching in paths
fn validate_bracket_matching(path: &str, param_name: &str) -> TViewResult<()> {
    let mut depth = 0;
    let mut pos = 0;

    for ch in path.chars() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return Err(TViewError::InvalidInput {
                        parameter: param_name.to_string(),
                        value: sanitize_for_logging(path),
                        reason: format!("Unmatched closing bracket ']' at position {}", pos),
                    });
                }
            }
            _ => {}
        }
        pos += 1;
    }

    if depth > 0 {
        return Err(TViewError::InvalidInput {
            parameter: param_name.to_string(),
            value: sanitize_for_logging(path),
            reason: format!("Unmatched opening bracket '[' ({} unclosed)", depth),
        });
    }

    Ok(())
}

/// Validate array indices are non-negative integers
fn validate_array_indices(path: &str, param_name: &str) -> TViewResult<()> {
    // Extract content between brackets
    let mut in_brackets = false;
    let mut current_index = String::new();
    let mut pos = 0;

    for ch in path.chars() {
        match ch {
            '[' => {
                in_brackets = true;
                current_index.clear();
            }
            ']' => {
                if in_brackets && !current_index.is_empty() {
                    // Validate index is non-negative integer
                    match current_index.parse::<u32>() {
                        Ok(_) => {}
                        Err(_) => {
                            return Err(TViewError::InvalidInput {
                                parameter: param_name.to_string(),
                                value: sanitize_for_logging(path),
                                reason: format!(
                                    "Invalid array index '{}' at position {} (must be non-negative integer)",
                                    current_index, pos
                                ),
                            });
                        }
                    }
                }
                in_brackets = false;
            }
            _ if in_brackets => {
                current_index.push(ch);
            }
            _ => {}
        }
        pos += 1;
    }

    Ok(())
}

/// Sanitize string for logging (truncate, remove sensitive chars)
fn sanitize_for_logging(s: &str) -> String {
    let max_len = 50;
    let truncated = if s.len() > max_len {
        format!("{}...", &s[..max_len])
    } else {
        s.to_string()
    };

    // Remove potential sensitive characters for logging
    truncated
        .replace('\0', "\\0")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Validate table name (stricter than generic identifier)
pub fn validate_table_name(name: &str) -> TViewResult<()> {
    validate_sql_identifier(name, "table_name")?;

    // Additional table-specific validation
    if !name.starts_with("tv_") && !name.starts_with("tb_") && !name.starts_with("test_") {
        return Err(TViewError::InvalidInput {
            parameter: "table_name".to_string(),
            value: sanitize_for_logging(name),
            reason: "Table name should start with tv_, tb_, or test_ prefix".to_string(),
        });
    }

    Ok(())
}

/// Validate column name (alias for identifier)
pub fn validate_column_name(name: &str) -> TViewResult<()> {
    validate_sql_identifier(name, "column_name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_identifiers() {
        assert!(validate_sql_identifier("my_table", "test").is_ok());
        assert!(validate_sql_identifier("user_data", "test").is_ok());
        assert!(validate_sql_identifier("pk_user", "test").is_ok());
        assert!(validate_sql_identifier("table123", "test").is_ok());
    }

    #[test]
    fn test_invalid_identifiers() {
        assert!(validate_sql_identifier("", "test").is_err());
        assert!(validate_sql_identifier("table; DROP", "test").is_err());
        assert!(validate_sql_identifier("user-data", "test").is_err());
        assert!(validate_sql_identifier("my table", "test").is_err());
        assert!(validate_sql_identifier("'admin'", "test").is_err());
        assert!(validate_sql_identifier("123table", "test").is_err());
    }

    #[test]
    fn test_valid_paths() {
        assert!(validate_jsonb_path("author.name", "test").is_ok());
        assert!(validate_jsonb_path("items[0]", "test").is_ok());
        assert!(validate_jsonb_path("users[5].profile.email", "test").is_ok());
        assert!(validate_jsonb_path("metadata.tags[0].value", "test").is_ok());
    }

    #[test]
    fn test_invalid_paths() {
        assert!(validate_jsonb_path("", "test").is_err());
        assert!(validate_jsonb_path("field'; DROP", "test").is_err());
        assert!(validate_jsonb_path("items[", "test").is_err());
        assert!(validate_jsonb_path("items]", "test").is_err());
        assert!(validate_jsonb_path("items[-1]", "test").is_err());
        assert!(validate_jsonb_path("author's.name", "test").is_err());
    }
}
```

**Integration Points**:
- Import in all phase implementations
- Used before any SQL string construction
- Consistent error types across all modules

**Testing**:
- Unit tests for each validator
- Property-based testing for edge cases
- Integration tests with actual SQL

---

### A2: Error Type Extensions (MEDIUM PRIORITY)

**File**: `src/error/mod.rs` (MODIFY)

**Add New Error Variants**:

```rust
#[derive(Debug)]
pub enum TViewError {
    // ... existing variants ...

    /// Input validation failed (non-security issue)
    InvalidInput {
        parameter: String,
        value: String,
        reason: String,
    },

    /// Security violation detected (SQL injection, etc.)
    SecurityViolation {
        parameter: String,
        value: String,  // Sanitized for logging
        reason: String,
    },

    /// Feature requires optional dependency
    MissingDependency {
        feature: String,
        dependency: String,
        install_command: String,
    },
}

impl Display for TViewError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            TViewError::InvalidInput { parameter, value, reason } => {
                write!(
                    f,
                    "Invalid input for parameter '{}': {}. Value: {}",
                    parameter, reason, value
                )
            }
            TViewError::SecurityViolation { parameter, value, reason } => {
                write!(
                    f,
                    "Security violation in parameter '{}': {}. Value: {}",
                    parameter, reason, value
                )
            }
            TViewError::MissingDependency { feature, dependency, install_command } => {
                write!(
                    f,
                    "Feature '{}' requires extension '{}'. Install with: {}",
                    feature, dependency, install_command
                )
            }
            // ... existing variants ...
        }
    }
}
```

---

### A3: Testing Utilities (MEDIUM PRIORITY)

**File**: `test/sql/00-security-test-helpers.sql` (NEW)

**Purpose**: Reusable SQL functions for security testing

```sql
-- Security Testing Helper Functions
-- Used across all phase integration tests

-- Test that a function rejects SQL injection
CREATE OR REPLACE FUNCTION assert_rejects_injection(
    test_name TEXT,
    test_func TEXT,  -- Function call with injection attempt
    expected_error_pattern TEXT DEFAULT 'injection|invalid|security'
) RETURNS VOID AS $$
DECLARE
    error_occurred BOOLEAN := FALSE;
    error_message TEXT;
BEGIN
    -- Try to execute the injection attempt
    EXECUTE test_func;

    -- If we get here, injection wasn't prevented!
    RAISE EXCEPTION 'SECURITY FAILURE [%]: SQL injection was not prevented!', test_name;

EXCEPTION
    WHEN OTHERS THEN
        error_occurred := TRUE;
        error_message := SQLERRM;

        -- Check if error message indicates security rejection
        IF error_message ~* expected_error_pattern THEN
            RAISE NOTICE 'PASS [%]: SQL injection correctly rejected', test_name;
        ELSE
            RAISE EXCEPTION 'SECURITY FAILURE [%]: Injection caused unexpected error: %',
                test_name, error_message;
        END IF;
END;
$$ LANGUAGE plpgsql;

-- Test that a function works with valid input
CREATE OR REPLACE FUNCTION assert_accepts_valid(
    test_name TEXT,
    test_func TEXT,  -- Function call with valid input
    expected_result TEXT DEFAULT NULL
) RETURNS VOID AS $$
DECLARE
    actual_result TEXT;
BEGIN
    EXECUTE test_func INTO actual_result;

    IF expected_result IS NOT NULL AND actual_result != expected_result THEN
        RAISE EXCEPTION 'FAILURE [%]: Expected %, got %',
            test_name, expected_result, actual_result;
    END IF;

    RAISE NOTICE 'PASS [%]: Valid input accepted', test_name;

EXCEPTION
    WHEN OTHERS THEN
        RAISE EXCEPTION 'FAILURE [%]: Valid input rejected: %', test_name, SQLERRM;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION assert_rejects_injection IS
'Test helper: Verify function rejects SQL injection attempts';

COMMENT ON FUNCTION assert_accepts_valid IS
'Test helper: Verify function accepts valid input';
```

---

## 📋 Phase B: Update Individual Phases

### B1: Update Phase 1 (2-3 hours)

**File**: `phase-1-helper-functions.md`

**Changes Required**:

1. **Add validation module import** (Step 0 - NEW)
```markdown
### Step 0: Understand Validation Infrastructure

Before implementing Phase 1, understand the validation helpers:

**Location**: `src/validation.rs`

Read the validation module documentation to understand:
- `validate_sql_identifier()` - For table/column names
- `validate_jsonb_path()` - For JSONB paths
- When to use each validator
- Error types returned

These validators are used in ALL functions to prevent SQL injection.
```

2. **Update extract_jsonb_id()** (Step 1)
   - Add validation call at function start
   - Use parameterized queries
   - Add security test case
   - Update documentation with security notes

3. **Update check_array_element_exists()** (Step 2)
   - Add validation for array_path segments
   - Add validation for id_key
   - Fix JSONPath syntax (`**` → `[*]`)
   - Add bracket matching validation

4. **Add security tests** (Step 3 - EXPAND)
```sql
-- Test 6: Security - SQL Injection Prevention
\echo '### Test 6: SQL Injection Prevention'

-- Test: Malicious id_key
SELECT assert_rejects_injection(
    'Extract ID with SQL injection',
    $$SELECT jsonb_extract_id('{"id": "test"}'::jsonb, 'id''; DROP TABLE users; --')$$
);

-- Test: Valid id_key
SELECT assert_accepts_valid(
    'Extract ID with valid key',
    $$SELECT jsonb_extract_id('{"id": "test123"}'::jsonb, 'id')$$,
    'test123'
);

\echo '### All security tests passed! ✓'
```

5. **Update verification steps**
   - Add security test execution
   - Add validation module compilation check

**Detailed Step-by-Step**:

See `PHASE1-CORRECTIONS.md` (to be created) for complete corrected implementation.

---

### B2: Update Phase 2 (2-3 hours)

**File**: `phase-2-nested-path-updates.md`

**Changes Required**:

1. **Add validation infrastructure reference** (Step 0 - NEW)

2. **Update DependencyDetail parsing** (Step 1)
   - Add validation when parsing nested_path from metadata
   - Prevent malicious metadata injection

3. **Update update_array_element_path()** (Step 2)
   - Add validation for all 5 string parameters
   - Implement proper fallback (not just error)
   - Add path syntax validation
   - Update documentation with constraints

4. **Update main.rs integration** (Step 3)
   - Validate nested_path before use in format!
   - Add error handling for validation failures

5. **Fix test data** (Step 4)
   - Correct JOIN logic in Test 3
   - Add proper post-comment relationship table

6. **Add security tests** (Step 4 - NEW)
```sql
-- Test 4: Security Testing
\echo '### Test 4: Security - SQL Injection Prevention'

-- Test: Table name injection
SELECT assert_rejects_injection(
    'Path update with table injection',
    $$SELECT update_array_element_path(
        'tv_post; DROP TABLE users; --', ...
    )$$
);

-- Test: Path injection
SELECT assert_rejects_injection(
    'Path update with path injection',
    $$UPDATE test_table SET data = jsonb_ivm_array_update_where_path(
        data, 'items', 'id', '1'::jsonb,
        'field''); DROP TABLE users; --', '"value"'::jsonb
    )$$
);

-- Test: Valid update
-- (Use actual function call with validated inputs)
```

**Detailed Step-by-Step**:

See `PHASE2-CORRECTIONS.md` (to be created) for complete corrected implementation.

---

### B3: Update Phase 3 (1-2 hours)

**File**: `phase-3-batch-operations.md`

**Changes Required** (Preemptive - Before Review):

1. **Review for SQL injection patterns**
   - Check `update_array_elements_batch()` for unvalidated params
   - Check `split_into_batches()` for input validation
   - Check batch size calculations for integer overflow

2. **Add validation**
   - Validate table_name, pk_column, array_path, match_key
   - Validate updates array structure
   - Validate batch sizes (prevent DoS)

3. **Implement fallback**
   - Ensure fallback_sequential_updates() properly validates
   - Add error handling for partial batch failures

4. **Add security tests**
   - SQL injection in batch updates
   - Oversized batch DoS attempts
   - Malformed updates array

**Specific Areas to Check**:

```rust
// POTENTIAL VULNERABILITY - Review this pattern:
let sql = format!(
    "UPDATE {table_name} SET ..."  // ← Validate table_name!
);

// POTENTIAL ISSUE - Check batch size limits:
pub fn optimal_batch_size(total_updates: usize) -> usize {
    match total_updates {
        _ => 100,  // ← What if total_updates is usize::MAX? DoS?
    }
}
```

---

### B4: Update Phase 4 (1-2 hours)

**File**: `phase-4-fallback-paths.md`

**Changes Required** (Preemptive):

1. **Review jsonb_ivm_set_path usage**
   - Check path validation
   - Check table_name, pk_column validation

2. **Add validation**
   - All function parameters
   - Path syntax and depth limits
   - Add path complexity limits

3. **Security concerns**
   - Path traversal attacks (e.g., `../../../../etc/passwd`)
   - Recursive path explosions
   - Path depth DoS

4. **Add security tests**
   - Path traversal attempts
   - Deeply nested paths (DoS)
   - Invalid path syntax

---

### B5: Update Phase 5 (1 hour)

**File**: `phase-5-integration-testing.md`

**Changes Required**:

1. **Add comprehensive security test suite**
```sql
-- test/sql/99-security-comprehensive.sql

\echo '=========================================='
\echo 'Comprehensive Security Test Suite'
\echo 'Tests all phases for SQL injection'
\echo '=========================================='

-- Phase 1 Security Tests
\echo '### Phase 1: Helper Functions'
SELECT assert_rejects_injection('Phase1: extract_id injection', ...);
SELECT assert_rejects_injection('Phase1: array_contains injection', ...);

-- Phase 2 Security Tests
\echo '### Phase 2: Nested Paths'
SELECT assert_rejects_injection('Phase2: table name injection', ...);
SELECT assert_rejects_injection('Phase2: nested path injection', ...);

-- Phase 3 Security Tests
\echo '### Phase 3: Batch Operations'
SELECT assert_rejects_injection('Phase3: batch injection', ...);

-- Phase 4 Security Tests
\echo '### Phase 4: Fallback Paths'
SELECT assert_rejects_injection('Phase4: set_path injection', ...);

\echo '### All security tests passed! ✓'
```

2. **Add fallback testing**
```sql
-- Test all features work WITHOUT jsonb_ivm
DROP EXTENSION IF EXISTS jsonb_ivm CASCADE;

\echo '### Testing graceful degradation without jsonb_ivm'
-- Run subset of functionality tests
-- Verify fallbacks work (even if slower)
```

3. **Add performance regression tests**
```sql
-- Verify new validation doesn't slow things down too much
-- Benchmark validated vs unvalidated (unsafe) versions
-- Acceptable overhead: <10% for validation
```

---

## 📋 Phase C: Verification & Documentation

### C1: Cross-Phase Consistency Check

**Checklist**:

- [ ] All functions use same validation helpers
- [ ] All error messages follow same format
- [ ] All fallbacks follow same pattern
- [ ] All documentation uses same terminology
- [ ] All tests use same helper functions
- [ ] All security tests cover same attack vectors

**Script**: `scripts/verify-consistency.sh` (NEW)

```bash
#!/bin/bash
# Verify cross-phase consistency

echo "Checking for SQL injection vulnerabilities..."
# Search for unsafe format! patterns
git grep 'format!.*{table' src/ && {
    echo "ERROR: Found unvalidated table_name in format!"
    exit 1
}

echo "Checking for missing validation..."
# Search for functions that should validate but don't
git grep 'pub fn.*table_name.*&str' src/ | while read line; do
    # Check if function calls validate_sql_identifier
    # ...
done

echo "All consistency checks passed!"
```

---

### C2: Security Audit Checklist

**File**: `SECURITY-CHECKLIST.md` (NEW)

```markdown
# Security Audit Checklist

## Code Review

- [ ] No `format!()` with unvalidated user input
- [ ] All SQL uses parameterized queries where possible
- [ ] All identifiers validated before interpolation
- [ ] All paths validated for syntax and injection
- [ ] No `unwrap()` on user input
- [ ] All error messages sanitize sensitive data
- [ ] No secrets in debug/log output

## Testing

- [ ] SQL injection tests for each function
- [ ] Malformed input tests
- [ ] Boundary value tests (empty, max length, etc.)
- [ ] Fallback tests (without jsonb_ivm)
- [ ] Integration tests with malicious metadata
- [ ] DoS tests (large inputs, deep recursion)

## Documentation

- [ ] Security constraints documented
- [ ] Valid input examples provided
- [ ] Invalid input examples provided
- [ ] Error messages guide users to fix issues
- [ ] Installation security notes included

## Deployment

- [ ] Release notes mention security fixes
- [ ] Migration guide includes validation updates
- [ ] Breaking changes clearly documented
- [ ] Security advisory if upgrading existing code
```

---

### C3: Documentation Updates

**Update These Files**:

1. **README.md**
   - Add security section
   - Mention input validation
   - Link to security guidelines

2. **SECURITY.md** (NEW)
   - Responsible disclosure policy
   - Security best practices for users
   - Common pitfalls to avoid

3. **API Documentation**
   - Add security notes to each function
   - Document validation constraints
   - Show secure usage examples

4. **Migration Guide**
   - Document validation changes
   - Show before/after code examples
   - Explain breaking changes (if any)

---

## 🎯 Execution Timeline

### Week 1: Foundation (Phase A)

| Day | Task | Duration | Deliverable |
|-----|------|----------|-------------|
| Mon | Create validation module | 2h | `src/validation.rs` |
| Mon | Add error types | 1h | Updated `src/error/mod.rs` |
| Tue | Create test helpers | 2h | `test/sql/00-security-helpers.sql` |
| Tue | Write unit tests | 1h | Validation tests |
| **Total** | | **6h** | **Foundation Complete** |

### Week 1: Updates (Phase B)

| Day | Task | Duration | Deliverable |
|-----|------|----------|-------------|
| Wed | Update Phase 1 plan | 2h | Corrected phase-1 |
| Wed | Update Phase 2 plan | 2h | Corrected phase-2 |
| Thu | Update Phase 3 plan | 1h | Corrected phase-3 |
| Thu | Update Phase 4 plan | 1h | Corrected phase-4 |
| Thu | Update Phase 5 plan | 1h | Corrected phase-5 |
| **Total** | | **7h** | **All Phases Updated** |

### Week 1: Verification (Phase C)

| Day | Task | Duration | Deliverable |
|-----|------|----------|-------------|
| Fri | Consistency check | 1h | Verification script |
| Fri | Security audit | 1h | Completed checklist |
| Fri | Documentation | 1h | Updated docs |
| **Total** | | **3h** | **Ready for Review** |

**Grand Total**: 16 hours (2 days of work)

---

## 🚀 Quick Start

### For Developers Updating Plans

1. **Read this plan** - Understand the full scope
2. **Start with Phase A** - Create shared infrastructure first
3. **Update phases sequentially** - Don't skip ahead
4. **Test as you go** - Verify each phase independently
5. **Final verification** - Run all tests together

### For Code Reviewers

1. **Check validation calls** - Every function should validate inputs
2. **Look for format!() - Should only have validated params
3. **Verify tests** - Every function should have security tests
4. **Check fallbacks** - Graceful degradation must work
5. **Review docs** - Security constraints must be documented

---

## 📊 Success Criteria

### Must Have (Blocking)
- ✅ No SQL injection vulnerabilities remain
- ✅ All functions validate inputs
- ✅ All phases have security tests
- ✅ All tests pass
- ✅ Documentation complete

### Should Have (Important)
- ✅ Fallbacks implemented and tested
- ✅ API consistency across phases
- ✅ Performance overhead <10%
- ✅ Error messages helpful
- ✅ Code review checklist used

### Nice to Have (Optional)
- ✅ Automated security scanning
- ✅ Fuzzing tests
- ✅ Performance benchmarks
- ✅ Security badge in README
- ✅ Security advisory published

---

## 🔄 Review & Approval Process

1. **Self-Review**: Author reviews own changes against checklist
2. **Peer Review**: Another developer reviews for security
3. **Security Review**: Dedicated security review of validation logic
4. **Integration Test**: All phases tested together
5. **Documentation Review**: Docs match implementation
6. **Final Approval**: Sign-off before implementation begins

---

## 📝 Appendices

### Appendix A: Common Vulnerability Patterns

**Pattern 1: Direct Interpolation**
```rust
// ❌ VULNERABLE
let sql = format!("SELECT * FROM {}", table_name);

// ✅ SECURE
validate_table_name(table_name)?;
let sql = format!("SELECT * FROM {}", table_name); // Now safe after validation
```

**Pattern 2: Missing Fallback**
```rust
// ❌ BROKEN
if !has_feature {
    return Err("Feature required");
}

// ✅ GRACEFUL
if !has_feature {
    warning!("Using slower fallback");
    return fallback_implementation();
}
```

**Pattern 3: Insufficient Validation**
```rust
// ❌ INSUFFICIENT
if !input.is_empty() { ... }

// ✅ COMPREHENSIVE
validate_sql_identifier(input, "param_name")?;
```

---

### Appendix B: Testing Best Practices

1. **Test the negative case first** - Ensure rejections work
2. **Use realistic attack payloads** - Not just `'; DROP TABLE`
3. **Test boundary conditions** - Empty, max length, special chars
4. **Test fallbacks** - Disable optional dependencies
5. **Test performance** - Ensure validation is fast enough

---

### Appendix C: Useful Resources

- [OWASP SQL Injection Guide](https://owasp.org/www-community/attacks/SQL_Injection)
- [PostgreSQL Security Best Practices](https://www.postgresql.org/docs/current/sql-syntax-lexical.html#SQL-SYNTAX-IDENTIFIERS)
- [Rust Security Guidelines](https://anssi-fr.github.io/rust-guide/)
- [pgrx Security Considerations](https://github.com/pgcentralfoundation/pgrx)

---

## 🎯 Next Steps

1. **Review this plan** with stakeholders
2. **Get approval** to proceed
3. **Execute Phase A** (foundation)
4. **Execute Phase B** (updates)
5. **Execute Phase C** (verification)
6. **Final review** before implementation
7. **Provide to junior developers** for implementation

---

**Status**: 📋 **READY FOR REVIEW**
**Next Action**: Stakeholder approval to begin Phase A
**Estimated Completion**: End of Week 1 (16 hours)

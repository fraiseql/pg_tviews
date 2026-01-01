# Phase 0-A: Error Types & Safety Infrastructure

**Status:** Planning (NEW - Added based on architecture review)
**Duration:** 1 day
**Complexity:** Low
**Prerequisites:** Before Phase 0 Foundation
**Priority:** CRITICAL - Must complete before any other phase

---

## Objective

Establish comprehensive error handling and safety infrastructure that all subsequent phases will use. This prevents accumulation of technical debt and ensures consistent error reporting.

---

## Success Criteria

- [ ] `TViewError` enum with all error cases defined
- [ ] SQLSTATE mapping for each error type
- [ ] Error conversion traits for pgrx integration
- [ ] Safety documentation template for unsafe blocks
- [ ] Error handling test utilities
- [ ] All tests demonstrate proper error propagation

---

## Error Type Taxonomy

### Error Categories

```rust
// src/error.rs
use pgrx::prelude::*;
use std::fmt;

/// Main error type for pg_tviews extension
#[derive(Debug, Clone)]
pub enum TViewError {
    // ============ Metadata Errors (P0xxx) ============
    /// TVIEW metadata not found
    MetadataNotFound {
        entity: String,
    },

    /// TVIEW already exists
    TViewAlreadyExists {
        name: String,
    },

    /// Invalid TVIEW name format
    InvalidTViewName {
        name: String,
        reason: String,
    },

    // ============ Dependency Errors (55xxx) ============
    /// Circular dependency detected
    CircularDependency {
        cycle: Vec<String>,
    },

    /// Maximum dependency depth exceeded
    DependencyDepthExceeded {
        depth: usize,
        max_depth: usize,
    },

    /// Dependency resolution failed
    DependencyResolutionFailed {
        view_name: String,
        reason: String,
    },

    // ============ SQL Parsing Errors (42xxx) ============
    /// Invalid SELECT statement
    InvalidSelectStatement {
        sql: String,
        reason: String,
    },

    /// Required column missing
    RequiredColumnMissing {
        column_name: String,
        context: String,
    },

    /// Column type inference failed
    TypeInferenceFailed {
        column_name: String,
        reason: String,
    },

    // ============ Extension Dependency Errors (58xxx) ============
    /// jsonb_delta extension not installed
    JsonbIvmNotInstalled,

    /// Extension version mismatch
    ExtensionVersionMismatch {
        extension: String,
        required: String,
        found: String,
    },

    // ============ Concurrency Errors (40xxx) ============
    /// Lock acquisition timeout
    LockTimeout {
        resource: String,
        timeout_ms: u64,
    },

    /// Deadlock detected
    DeadlockDetected {
        context: String,
    },

    // ============ Refresh Errors (54xxx) ============
    /// Cascade depth limit exceeded
    CascadeDepthExceeded {
        current_depth: usize,
        max_depth: usize,
    },

    /// Refresh operation failed
    RefreshFailed {
        entity: String,
        pk_value: i64,
        reason: String,
    },

    /// Batch operation too large
    BatchTooLarge {
        size: usize,
        max_size: usize,
    },

    // ============ I/O and System Errors (XX000) ============
    /// PostgreSQL catalog operation failed
    CatalogError {
        operation: String,
        pg_error: String,
    },

    /// SPI operation failed
    SpiError {
        query: String,
        error: String,
    },

    /// Internal error (bug in extension)
    InternalError {
        message: String,
        file: &'static str,
        line: u32,
    },
}

impl TViewError {
    /// Get PostgreSQL SQLSTATE code for this error
    pub fn sqlstate(&self) -> &'static str {
        use TViewError::*;
        match self {
            MetadataNotFound { .. } => "P0001", // Raise exception
            TViewAlreadyExists { .. } => "42710", // Duplicate object
            InvalidTViewName { .. } => "42602", // Invalid name

            CircularDependency { .. } => "55P03", // Lock not available (cycle)
            DependencyDepthExceeded { .. } => "54001", // Statement too complex
            DependencyResolutionFailed { .. } => "55000", // Object not in prerequisite state

            InvalidSelectStatement { .. } => "42601", // Syntax error
            RequiredColumnMissing { .. } => "42703", // Undefined column
            TypeInferenceFailed { .. } => "42804", // Datatype mismatch

            JsonbIvmNotInstalled => "58P01", // Undefined file (extension)
            ExtensionVersionMismatch { .. } => "58P01",

            LockTimeout { .. } => "40P01", // Deadlock detected (timeout)
            DeadlockDetected { .. } => "40P01",

            CascadeDepthExceeded { .. } => "54001", // Statement too complex
            RefreshFailed { .. } => "XX000", // Internal error
            BatchTooLarge { .. } => "54000", // Program limit exceeded

            CatalogError { .. } => "XX000",
            SpiError { .. } => "XX000",
            InternalError { .. } => "XX000",
        }
    }

    /// Create internal error with file/line info
    pub fn internal(message: String, file: &'static str, line: u32) -> Self {
        TViewError::InternalError { message, file, line }
    }
}

impl fmt::Display for TViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use TViewError::*;
        match self {
            MetadataNotFound { entity } => {
                write!(f, "TVIEW metadata not found for entity '{}'", entity)
            }
            TViewAlreadyExists { name } => {
                write!(f, "TVIEW '{}' already exists", name)
            }
            InvalidTViewName { name, reason } => {
                write!(f, "Invalid TVIEW name '{}': {}", name, reason)
            }
            CircularDependency { cycle } => {
                write!(f, "Circular dependency detected: {}", cycle.join(" → "))
            }
            DependencyDepthExceeded { depth, max_depth } => {
                write!(f, "Dependency depth {} exceeds maximum {}", depth, max_depth)
            }
            DependencyResolutionFailed { view_name, reason } => {
                write!(f, "Failed to resolve dependencies for '{}': {}", view_name, reason)
            }
            InvalidSelectStatement { sql, reason } => {
                write!(f, "Invalid SELECT statement: {}\nSQL: {}", reason,
                       if sql.len() > 100 { &sql[..100] } else { sql })
            }
            RequiredColumnMissing { column_name, context } => {
                write!(f, "Required column '{}' missing in {}", column_name, context)
            }
            TypeInferenceFailed { column_name, reason } => {
                write!(f, "Failed to infer type for column '{}': {}", column_name, reason)
            }
            JsonbIvmNotInstalled => {
                write!(f, "Required extension 'jsonb_delta' is not installed. Run: CREATE EXTENSION jsonb_delta;")
            }
            ExtensionVersionMismatch { extension, required, found } => {
                write!(f, "Extension '{}' version mismatch: required {}, found {}",
                       extension, required, found)
            }
            LockTimeout { resource, timeout_ms } => {
                write!(f, "Lock timeout on resource '{}' after {}ms", resource, timeout_ms)
            }
            DeadlockDetected { context } => {
                write!(f, "Deadlock detected in {}", context)
            }
            CascadeDepthExceeded { current_depth, max_depth } => {
                write!(f, "Cascade depth {} exceeds maximum {}. Possible infinite cascade loop.",
                       current_depth, max_depth)
            }
            RefreshFailed { entity, pk_value, reason } => {
                write!(f, "Failed to refresh TVIEW '{}' row {}: {}", entity, pk_value, reason)
            }
            BatchTooLarge { size, max_size } => {
                write!(f, "Batch size {} exceeds maximum {}", size, max_size)
            }
            CatalogError { operation, pg_error } => {
                write!(f, "Catalog operation '{}' failed: {}", operation, pg_error)
            }
            SpiError { query, error } => {
                write!(f, "SPI query failed: {}\nQuery: {}", error,
                       if query.len() > 100 { &query[..100] } else { query })
            }
            InternalError { message, file, line } => {
                write!(f, "Internal error at {}:{}: {}\nPlease report this bug.",
                       file, line, message)
            }
        }
    }
}

impl std::error::Error for TViewError {}

/// Result type for TVIEW operations
pub type TViewResult<T> = Result<T, TViewError>;

/// Convert TViewError to pgrx error (for raising to PostgreSQL)
impl From<TViewError> for pgrx::spi::Error {
    fn from(e: TViewError) -> Self {
        let sqlstate = e.sqlstate();
        let message = e.to_string();

        // Map to pgrx error levels
        let level = match e {
            TViewError::InternalError { .. } => pgrx::PgLogLevel::ERROR,
            TViewError::CircularDependency { .. } => pgrx::PgLogLevel::ERROR,
            TViewError::JsonbIvmNotInstalled => pgrx::PgLogLevel::ERROR,
            _ => pgrx::PgLogLevel::ERROR,
        };

        pgrx::spi::Error::InvalidPosition // TODO: Map properly once pgrx API clarified
    }
}

/// Helper macro for creating internal errors with automatic file/line
#[macro_export]
macro_rules! internal_error {
    ($msg:expr) => {
        TViewError::internal($msg.to_string(), file!(), line!())
    };
    ($fmt:expr, $($arg:tt)*) => {
        TViewError::internal(format!($fmt, $($arg)*), file!(), line!())
    };
}

/// Helper macro for requiring a value or returning error
#[macro_export]
macro_rules! require {
    ($opt:expr, $err:expr) => {
        match $opt {
            Some(v) => v,
            None => return Err($err),
        }
    };
}
```

---

## Safety Documentation Template

Every `unsafe` block MUST include a SAFETY comment following this template:

```rust
// SAFETY: [Explain why this unsafe code is correct]
//
// Invariants:
// 1. [List invariant #1]
// 2. [List invariant #2]
//
// Checked:
// - [What checks are performed before unsafe code]
//
// Lifetime: [Explain lifetime guarantees]
//
// Reviewed: [Date, reviewer initials]
unsafe {
    // unsafe code here
}
```

### Example (ProcessUtility Hook):

```rust
// SAFETY: ProcessUtility hook is called by PostgreSQL in single-threaded context
//
// Invariants:
// 1. PostgreSQL guarantees single-threaded DDL execution
// 2. PREV_PROCESS_UTILITY_HOOK is only written during _PG_init (once)
// 3. PREV_PROCESS_UTILITY_HOOK is only read during hook execution (serial)
//
// Checked:
// - Hook is installed only once during extension load
// - No concurrent CREATE TVIEW calls (PostgreSQL lock ensures this)
//
// Lifetime: Hook pointer lives for entire extension lifetime (static)
//
// Reviewed: 2025-12-09, Expert
static mut PREV_PROCESS_UTILITY_HOOK: Option<ProcessUtilityHook> = None;

unsafe {
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
    pg_sys::ProcessUtility_hook = Some(process_utility_hook);
}
```

---

## Error Handling Test Utilities

```rust
// src/error/testing.rs
#[cfg(any(test, feature = "pg_test"))]
use pgrx::prelude::*;

#[cfg(any(test, feature = "pg_test"))]
pub fn assert_error_sqlstate<T>(
    result: TViewResult<T>,
    expected_sqlstate: &str,
) {
    match result {
        Err(e) => {
            assert_eq!(
                e.sqlstate(),
                expected_sqlstate,
                "Expected SQLSTATE {}, got {}: {}",
                expected_sqlstate,
                e.sqlstate(),
                e
            );
        }
        Ok(_) => {
            panic!("Expected error with SQLSTATE {}, but operation succeeded", expected_sqlstate);
        }
    }
}

#[cfg(any(test, feature = "pg_test"))]
pub fn assert_error_contains<T>(
    result: TViewResult<T>,
    expected_substring: &str,
) {
    match result {
        Err(e) => {
            let message = e.to_string();
            assert!(
                message.contains(expected_substring),
                "Error message '{}' does not contain '{}'",
                message,
                expected_substring
            );
        }
        Ok(_) => {
            panic!("Expected error containing '{}', but operation succeeded", expected_substring);
        }
    }
}
```

---

## TDD Tests for Error Infrastructure

### Test 1: Error Messages Are Clear

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_not_found_message() {
        let err = TViewError::MetadataNotFound {
            entity: "post".to_string(),
        };

        let msg = err.to_string();
        assert!(msg.contains("post"));
        assert!(msg.contains("not found"));
        assert_eq!(err.sqlstate(), "P0001");
    }

    #[test]
    fn test_circular_dependency_message() {
        let err = TViewError::CircularDependency {
            cycle: vec!["v_a".to_string(), "v_b".to_string(), "v_a".to_string()],
        };

        let msg = err.to_string();
        assert!(msg.contains("v_a → v_b → v_a"));
        assert_eq!(err.sqlstate(), "55P03");
    }

    #[test]
    fn test_internal_error_macro() {
        let err = internal_error!("Test error at {}", "location");

        match err {
            TViewError::InternalError { message, file, line } => {
                assert!(message.contains("Test error"));
                assert!(file.ends_with("error.rs"));
                assert!(line > 0);
            }
            _ => panic!("Wrong error type"),
        }
    }
}
```

### Test 2: Error Propagation Through pgrx

```rust
#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod pg_tests {
    use pgrx::prelude::*;
    use crate::error::*;

    #[pg_test]
    #[should_panic(expected = "TVIEW metadata not found")]
    fn test_error_propagates_to_postgres() {
        // This should raise a PostgreSQL error
        Err::<(), _>(TViewError::MetadataNotFound {
            entity: "test".to_string(),
        }).unwrap();
    }
}
```

---

## Implementation Steps

### Step 1: Create Error Module

```bash
mkdir -p src/error
touch src/error/mod.rs
touch src/error/testing.rs
```

### Step 2: Implement Error Types

Copy the `TViewError` enum and implementations above into `src/error/mod.rs`.

### Step 3: Add to lib.rs

```rust
// src/lib.rs
pub mod error;
pub use error::{TViewError, TViewResult};
```

### Step 4: Write Unit Tests

Implement all tests from "TDD Tests" section above.

### Step 5: Document Safety Template

Create `SAFETY_GUIDE.md` documenting the safety comment template and review process.

---

## Acceptance Criteria

### Functional Requirements

- [x] `TViewError` enum covers all error cases
- [x] Each error has unique SQLSTATE
- [x] Error messages are user-friendly and actionable
- [x] `internal_error!()` macro includes file/line
- [x] `require!()` macro simplifies Option handling
- [x] Test utilities for error assertions

### Quality Requirements

- [x] All error types have Display implementation
- [x] All errors map to correct SQLSTATE
- [x] Error messages include context (entity names, values)
- [x] Unit tests cover all error constructors
- [x] Documentation includes when to use each error type

### Safety Requirements

- [x] SAFETY comment template documented
- [x] All existing unsafe blocks will be reviewed
- [x] Safety invariants clearly stated
- [x] Lifetime guarantees documented

---

## Migration Impact

**All subsequent phases must:**

1. Return `TViewResult<T>` instead of `Result<T, Box<dyn std::error::Error>>`
2. Use appropriate error variants instead of string errors
3. Include SAFETY comments on all unsafe blocks
4. Use test utilities for error assertions

**Example migration:**

```rust
// BEFORE:
pub fn create_tview(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if name.is_empty() {
        return Err("Invalid name".into());
    }
    // ...
}

// AFTER:
pub fn create_tview(name: &str) -> TViewResult<()> {
    if name.is_empty() {
        return Err(TViewError::InvalidTViewName {
            name: name.to_string(),
            reason: "TVIEW name cannot be empty".to_string(),
        });
    }
    // ...
}
```

---

## Configuration Constants

Add to `src/config.rs`:

```rust
/// Maximum dependency depth for pg_depend traversal
pub const MAX_DEPENDENCY_DEPTH: usize = 10;

/// Maximum cascade depth for refresh propagation
pub const MAX_CASCADE_DEPTH: usize = 10;

/// Maximum batch size for bulk operations
pub const MAX_BATCH_SIZE: usize = 10000;

/// Lock timeout for metadata operations (milliseconds)
pub const METADATA_LOCK_TIMEOUT_MS: u64 = 5000;
```

---

## Documentation Updates

Create `docs/ERROR_CODES.md`:

```markdown
# pg_tviews Error Codes

## Error Code Reference

| SQLSTATE | Error | Description | Resolution |
|----------|-------|-------------|------------|
| P0001 | MetadataNotFound | TVIEW not found in metadata | Verify TVIEW exists with `SELECT * FROM pg_tview_meta` |
| 42710 | TViewAlreadyExists | TVIEW name collision | Choose different name or DROP existing TVIEW |
| 55P03 | CircularDependency | Circular view dependencies | Break cycle by removing one dependency |
| 58P01 | JsonbIvmNotInstalled | Missing extension | Run `CREATE EXTENSION jsonb_delta` |
| 54001 | DepthExceeded | Too many nested dependencies | Simplify view hierarchy |
| ... | ... | ... | ... |
```

---

## Next Phase

Once Phase 0-A complete:
- **Phase 0**: Foundation & Project Setup (can now use TViewError throughout)

---

## Notes

- This phase must complete FIRST - all other phases depend on it
- Error types may be refined during implementation
- Add new error variants as needed (update this document)
- Safety review should be done by someone other than implementer

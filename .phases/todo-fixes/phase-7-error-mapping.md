# Phase 7: Improve SPI Error Mapping

## Objective

Replace the placeholder error mapping in `src/error/mod.rs:414` with proper error type conversion once pgrx's error API stabilizes.

## Context

Currently, `src/error/mod.rs:414` has:

```rust
pgrx::spi::Error::InvalidPosition // TODO: Map properly once pgrx API clarified
```

This is used when converting `TViewError` to pgrx's SPI error type for propagation to PostgreSQL. The current mapping is imprecise.

## Current State

The error conversion exists in the `From<TViewError> for spi::Error` implementation:

```rust
impl From<TViewError> for spi::Error {
    fn from(e: TViewError) -> Self {
        // Current: all errors map to InvalidPosition (incorrect)
        pgrx::spi::Error::InvalidPosition
    }
}
```

## Desired Behavior

Map `TViewError` variants to appropriate `spi::Error` variants:

| TViewError | Should Map To |
|------------|---------------|
| `MetadataNotFound` | Custom error message |
| `SpiError` | Preserve original SPI error |
| `ConfigError` | Custom error message |
| `DependencyDepthExceeded` | Custom error message |
| `CascadeDepthExceeded` | Custom error message |
| `PropagationDepthExceeded` | Custom error message |
| `SerializationError` | Custom error message |

## Files to Modify

| File | Changes |
|------|---------|
| `src/error/mod.rs` | Update `From<TViewError> for spi::Error` implementation |

## Implementation Steps

### Step 1: Research pgrx error API

Check current pgrx version's `spi::Error` enum variants:

```rust
// pgrx 0.16.x spi::Error variants (approximate):
pub enum Error {
    InvalidPosition,
    CursorNotFound,
    SpiError(String),
    // ... others
}
```

### Step 2: Implement proper mapping

```rust
impl From<TViewError> for spi::Error {
    fn from(e: TViewError) -> Self {
        // Use SpiError variant to pass through our error messages
        // This allows PostgreSQL to display meaningful error text
        match &e {
            TViewError::SpiError { query, error } => {
                // If we have an original SPI error, try to preserve context
                spi::Error::SpiError(format!("Query '{}' failed: {}", query, error))
            }
            _ => {
                // For all other errors, use Display formatting
                spi::Error::SpiError(e.to_string())
            }
        }
    }
}
```

### Step 3: Alternative - Use pgrx error! macro

If `spi::Error` doesn't provide good variants, use pgrx's `error!` macro directly:

```rust
// Instead of converting to spi::Error, raise PostgreSQL error directly
pub fn raise_as_pg_error(e: TViewError) -> ! {
    match e {
        TViewError::MetadataNotFound { entity } => {
            pgrx::error!("TVIEW metadata not found for entity '{}'", entity);
        }
        TViewError::SpiError { query, error } => {
            pgrx::error!("SPI query failed: {} (query: {})", error, query);
        }
        TViewError::ConfigError { message } => {
            pgrx::error!("Configuration error: {}", message);
        }
        TViewError::DependencyDepthExceeded { depth, max_depth } => {
            pgrx::error!(
                "Dependency depth {} exceeds maximum {}",
                depth, max_depth
            );
        }
        TViewError::CascadeDepthExceeded { current_depth, max_depth } => {
            pgrx::error!(
                "Cascade depth {} exceeds maximum {} (possible infinite loop)",
                current_depth, max_depth
            );
        }
        TViewError::PropagationDepthExceeded { max_depth, processed } => {
            pgrx::error!(
                "Propagation exceeded {} iterations ({} entities). Possible circular dependency.",
                max_depth, processed
            );
        }
        TViewError::SerializationError { context, message } => {
            pgrx::error!("Serialization error in {}: {}", context, message);
        }
        _ => {
            pgrx::error!("pg_tviews error: {}", e);
        }
    }
}
```

### Step 4: Add SQLSTATE codes

For better error handling in client applications, include SQLSTATE codes:

```rust
// Use pgrx::ereport! for full control
use pgrx::ereport;
use pgrx::PgSqlErrorCode;

pub fn raise_as_pg_error(e: TViewError) -> ! {
    match e {
        TViewError::MetadataNotFound { entity } => {
            ereport!(
                ERROR,
                PgSqlErrorCode::ERRCODE_UNDEFINED_OBJECT,
                "TVIEW metadata not found for entity '{}'",
                entity
            );
        }
        TViewError::DependencyDepthExceeded { .. } |
        TViewError::CascadeDepthExceeded { .. } |
        TViewError::PropagationDepthExceeded { .. } => {
            ereport!(
                ERROR,
                PgSqlErrorCode::ERRCODE_PROGRAM_LIMIT_EXCEEDED,
                "{}",
                e
            );
        }
        _ => {
            ereport!(
                ERROR,
                PgSqlErrorCode::ERRCODE_INTERNAL_ERROR,
                "pg_tviews: {}",
                e
            );
        }
    }
}
```

## Verification Commands

```bash
# Build check
cargo check --no-default-features --features pg18

# Run clippy
cargo clippy --no-default-features --features pg18 -- -D warnings

# Test error messages
cargo pgrx test pg18
```

## SQL Verification

```sql
-- Test metadata not found error
SELECT pg_tviews_refresh('nonexistent_entity', 1);
-- Should show: ERROR: TVIEW metadata not found for entity 'nonexistent_entity'

-- Test depth exceeded (would need to create circular dependency)
-- Should show: ERROR: Dependency depth X exceeds maximum Y
```

## Acceptance Criteria

- [ ] Error messages are meaningful to users
- [ ] SQLSTATE codes are appropriate
- [ ] Original error context preserved where possible
- [ ] No placeholder InvalidPosition errors
- [ ] Code compiles without warnings
- [ ] Clippy passes

## DO NOT

- Do not change error enum variants
- Do not lose error context in conversion
- Do not use panic! for recoverable errors
- Do not break existing error handling paths

## Priority

**Low** - This is a polish/UX improvement. The extension works correctly with the current placeholder mapping; errors are just less informative than they could be.

## Notes

This depends on pgrx's error API which has evolved between versions. Check the pgrx documentation for the installed version before implementing.

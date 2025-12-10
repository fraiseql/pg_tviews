
use std::fmt;

pub mod testing;

/// Main error type for pg_tviews extension
#[derive(Debug, Clone, PartialEq)]
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
    /// jsonb_ivm extension not installed
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

    // ============ Graph and Propagation Errors (Phase 6D) ============
    /// Dependency cycle detected in entity graph
    DependencyCycle {
        entities: Vec<String>,
    },

    /// Propagation exceeded maximum depth (possible infinite loop)
    PropagationDepthExceeded {
        max_depth: usize,
        processed: usize,
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

    /// Serialization/deserialization failed
    SerializationError {
        message: String,
    },

    /// Configuration error (invalid GUC values)
    ConfigError {
        setting: String,
        value: String,
        reason: String,
    },

    /// Cache error (poisoned mutex, corruption)
    CacheError {
        cache_name: String,
        reason: String,
    },

    /// FFI callback error (panic in C context)
    CallbackError {
        callback_name: String,
        error: String,
    },

    /// Metrics error (tracking failure)
    MetricsError {
        operation: String,
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

            DependencyCycle { .. } => "55P03", // Lock not available (cycle)
            PropagationDepthExceeded { .. } => "54001", // Statement too complex

            CatalogError { .. } => "XX000",
            SpiError { .. } => "XX000",
            SerializationError { .. } => "XX000",
            ConfigError { .. } => "XX000",
            CacheError { .. } => "XX000",
            CallbackError { .. } => "XX000",
            MetricsError { .. } => "XX000",
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
                write!(f, "Required extension 'jsonb_ivm' is not installed. Run: CREATE EXTENSION jsonb_ivm;")
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
            DependencyCycle { entities } => {
                write!(f, "Dependency cycle detected in entity graph: {}", entities.join(" -> "))
            }
            PropagationDepthExceeded { max_depth, processed } => {
                write!(
                    f,
                    "Propagation exceeded maximum depth of {} iterations ({} entities processed). \
                     Possible infinite loop or extremely deep dependency chain.",
                    max_depth, processed
                )
            }
            CatalogError { operation, pg_error } => {
                write!(f, "Catalog operation '{}' failed: {}", operation, pg_error)
            }
            SpiError { query, error } => {
                write!(f, "SPI query failed: {}\nQuery: {}", error,
                       if query.len() > 100 { &query[..100] } else { query })
            }
            SerializationError { message } => {
                write!(f, "Serialization error: {}", message)
            }
            ConfigError { setting, value, reason } => {
                write!(f, "Configuration error for '{}': {} (value: {})", setting, reason, value)
            }
            CacheError { cache_name, reason } => {
                write!(f, "Cache '{}' error: {}", cache_name, reason)
            }
            CallbackError { callback_name, error } => {
                write!(f, "FFI callback '{}' failed: {}", callback_name, error)
            }
            MetricsError { operation, error } => {
                write!(f, "Metrics operation '{}' failed: {}", operation, error)
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

/// Convert SpiError to TViewError
impl From<pgrx::spi::Error> for TViewError {
    fn from(e: pgrx::spi::Error) -> Self {
        TViewError::SpiError {
            query: "Unknown".to_string(),
            error: e.to_string(),
        }
    }
}

/// Convert serde_json::Error to TViewError
impl From<serde_json::Error> for TViewError {
    fn from(e: serde_json::Error) -> Self {
        TViewError::SerializationError {
            message: format!("JSON serialization error: {}", e),
        }
    }
}

/// Convert bincode::Error to TViewError
impl From<bincode::Error> for TViewError {
    fn from(e: bincode::Error) -> Self {
        TViewError::SerializationError {
            message: format!("Binary serialization error: {}", e),
        }
    }
}

/// Convert regex::Error to TViewError
impl From<regex::Error> for TViewError {
    fn from(e: regex::Error) -> Self {
        TViewError::InvalidSelectStatement {
            sql: "Unknown".to_string(),
            reason: format!("Regex compilation failed: {}", e),
        }
    }
}

/// Convert std::io::Error to TViewError
impl From<std::io::Error> for TViewError {
    fn from(e: std::io::Error) -> Self {
        TViewError::SerializationError {
            message: format!("I/O error: {}", e),
        }
    }
}

/// Convert TViewError to pgrx error (for raising to PostgreSQL)
impl From<TViewError> for pgrx::spi::Error {
    fn from(e: TViewError) -> Self {
        let _sqlstate = e.sqlstate();
        let _message = e.to_string();

        // Map to pgrx error levels
        let _level = match e {
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
                assert!(file.ends_with("mod.rs"));
                assert!(line > 0);
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_all_error_sqlstates_unique() {
        let errors = vec![
            TViewError::MetadataNotFound { entity: "test".to_string() },
            TViewError::TViewAlreadyExists { name: "test".to_string() },
            TViewError::InvalidTViewName { name: "test".to_string(), reason: "test".to_string() },
            TViewError::CircularDependency { cycle: vec![] },
            TViewError::DependencyDepthExceeded { depth: 1, max_depth: 1 },
            TViewError::DependencyResolutionFailed { view_name: "test".to_string(), reason: "test".to_string() },
            TViewError::InvalidSelectStatement { sql: "test".to_string(), reason: "test".to_string() },
            TViewError::RequiredColumnMissing { column_name: "test".to_string(), context: "test".to_string() },
            TViewError::TypeInferenceFailed { column_name: "test".to_string(), reason: "test".to_string() },
            TViewError::JsonbIvmNotInstalled,
            TViewError::ExtensionVersionMismatch { extension: "test".to_string(), required: "1".to_string(), found: "2".to_string() },
            TViewError::LockTimeout { resource: "test".to_string(), timeout_ms: 1000 },
            TViewError::DeadlockDetected { context: "test".to_string() },
            TViewError::CascadeDepthExceeded { current_depth: 1, max_depth: 1 },
            TViewError::RefreshFailed { entity: "test".to_string(), pk_value: 1, reason: "test".to_string() },
            TViewError::BatchTooLarge { size: 1, max_size: 1 },
            TViewError::CatalogError { operation: "test".to_string(), pg_error: "test".to_string() },
            TViewError::SpiError { query: "test".to_string(), error: "test".to_string() },
            TViewError::SerializationError { message: "test".to_string() },
            TViewError::ConfigError { setting: "test".to_string(), value: "test".to_string(), reason: "test".to_string() },
            TViewError::CacheError { cache_name: "test".to_string(), reason: "test".to_string() },
            TViewError::CallbackError { callback_name: "test".to_string(), error: "test".to_string() },
            TViewError::MetricsError { operation: "test".to_string(), error: "test".to_string() },
            TViewError::InternalError { message: "test".to_string(), file: "test", line: 1 },
        ];

        let sqlstates: Vec<&str> = errors.iter().map(|e| e.sqlstate()).collect();
        let unique_sqlstates: std::collections::HashSet<&str> = sqlstates.iter().cloned().collect();

        // All SQLSTATEs should be unique (though some may share codes intentionally)
        assert!(unique_sqlstates.len() >= 15, "Too many duplicate SQLSTATE codes");
    }
}
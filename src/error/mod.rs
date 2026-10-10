//! The errors `pg_tviews` raises, each with the SQLSTATE a client can catch.
//!
//! A [`TViewError`] reaches the client through [`ErrorReport`]: a `#[pg_extern]`
//! returns `Result<T, ErrorReport>` (pgrx reports any other error type as 22000),
//! and code that must raise in place calls [`TViewError::raise`]. Either way the
//! client sees the variant's SQLSTATE, its one-line message, and the detail and
//! hint it carries.

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::{PgLogLevel, PgSqlErrorCode};
use std::fmt;

#[cfg(test)]
mod reference;

/// An error `pg_tviews` raises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TViewError {
    /// No TVIEW is registered under this entity.
    MetadataNotFound { entity: String },

    /// The table or backing view a TVIEW needs is already taken.
    RelationExists { name: String },

    /// A function argument is invalid.
    InvalidInput { parameter: String, reason: String },

    /// `pg_tviews` cannot maintain a TVIEW with this definition.
    DefinitionRefused { reason: String },

    /// The definition's `pk_<entity>` column is not an integer.
    KeyTypeRefused { column: String, found: String },

    /// A statement would change a TVIEW table's columns, which its definition
    /// sets.
    ColumnDdlRefused { table: String, change: String },

    /// A user's index would take a name `pg_tviews` keeps for its own indexes on a
    /// TVIEW's table.
    IndexNameReserved { table: String, index: String },

    /// The current role may not do this.
    PermissionDenied { reason: String },

    /// TVIEWs read each other in a cycle.
    DependencyCycle { entities: Vec<String> },

    /// Views, TVIEWs or propagation nest deeper than the configured limit.
    DepthExceeded {
        what: &'static str,
        depth: usize,
        max_depth: usize,
    },

    /// The definition is not a single SELECT `pg_tviews` can read.
    InvalidSelectStatement { sql: String, reason: String },

    /// A column every TVIEW needs is missing from the definition.
    RequiredColumnMissing {
        column_name: String,
        context: String,
    },

    /// The `jsonb_delta` extension is not installed.
    JsonbDeltaMissing,

    /// The transaction queued more refreshes than `pg_tviews.max_queue_size`.
    QueueFull { size: usize, max_size: usize },

    /// The session or transaction is not in a state that allows this.
    WrongState { reason: String },

    /// Reading or writing the catalog failed (internal).
    CatalogError { operation: String, pg_error: String },

    /// A query `pg_tviews` runs failed (internal).
    SpiError { query: String, error: String },

    /// A stored value could not be decoded (internal).
    SerializationError { message: String },
}

impl TViewError {
    /// The SQLSTATE this error is raised with.
    #[must_use]
    pub const fn errcode(&self) -> PgSqlErrorCode {
        match self {
            Self::MetadataNotFound { .. } => PgSqlErrorCode::ERRCODE_UNDEFINED_OBJECT,
            Self::RelationExists { .. } => PgSqlErrorCode::ERRCODE_DUPLICATE_TABLE,
            Self::InvalidInput { .. } => PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
            Self::DefinitionRefused { .. } => PgSqlErrorCode::ERRCODE_FEATURE_NOT_SUPPORTED,
            Self::KeyTypeRefused { .. } => PgSqlErrorCode::ERRCODE_DATATYPE_MISMATCH,
            Self::ColumnDdlRefused { .. } => PgSqlErrorCode::ERRCODE_WRONG_OBJECT_TYPE,
            Self::IndexNameReserved { .. } => PgSqlErrorCode::ERRCODE_RESERVED_NAME,
            Self::PermissionDenied { .. } => PgSqlErrorCode::ERRCODE_INSUFFICIENT_PRIVILEGE,
            Self::DependencyCycle { .. } => PgSqlErrorCode::ERRCODE_INVALID_OBJECT_DEFINITION,
            Self::DepthExceeded { .. } => PgSqlErrorCode::ERRCODE_STATEMENT_TOO_COMPLEX,
            Self::InvalidSelectStatement { .. } => PgSqlErrorCode::ERRCODE_SYNTAX_ERROR,
            Self::RequiredColumnMissing { .. } => PgSqlErrorCode::ERRCODE_UNDEFINED_COLUMN,
            Self::JsonbDeltaMissing => PgSqlErrorCode::ERRCODE_UNDEFINED_FUNCTION,
            Self::QueueFull { .. } => PgSqlErrorCode::ERRCODE_PROGRAM_LIMIT_EXCEEDED,
            Self::WrongState { .. } => PgSqlErrorCode::ERRCODE_OBJECT_NOT_IN_PREREQUISITE_STATE,
            Self::CatalogError { .. } | Self::SpiError { .. } | Self::SerializationError { .. } => {
                PgSqlErrorCode::ERRCODE_INTERNAL_ERROR
            }
        }
    }

    /// The five-character SQLSTATE, e.g. `"42704"`.
    #[must_use]
    pub fn sqlstate(&self) -> String {
        sqlstate_text(self.errcode())
    }

    /// Supporting detail for the client, kept out of the one-line message.
    #[must_use]
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::InvalidSelectStatement { sql, .. } if !sql.is_empty() => Some(format!(
                "Definition: {}",
                crate::utils::truncate_chars(sql, 200)
            )),
            Self::SpiError { query, .. } if !query.is_empty() => Some(format!(
                "Query: {}",
                crate::utils::truncate_chars(query, 200)
            )),
            _ => None,
        }
    }

    /// What the client can do about it.
    #[must_use]
    pub fn hint(&self) -> Option<String> {
        match self {
            Self::MetadataNotFound { .. } => {
                Some("SELECT entity FROM tviews.pg_tview_meta lists the registered TVIEWs.".into())
            }
            Self::RelationExists { .. } => {
                Some("pg_tviews_create_or_replace() changes an existing TVIEW.".into())
            }
            Self::JsonbDeltaMissing => Some("CREATE EXTENSION jsonb_delta;".into()),
            Self::KeyTypeRefused { .. } => Some(
                "Key the rows on an integer column, and keep a uuid key in the id column.".into(),
            ),
            Self::ColumnDdlRefused { .. } => Some(
                "Change the definition with tviews.pg_tviews_create_or_replace(): the table \
                 follows it."
                    .into(),
            ),
            Self::IndexNameReserved { .. } => Some(
                "Give the index another name. pg_tviews creates its own indexes on a TVIEW \
                 (tviews.registry.managed_indexes lists them)."
                    .into(),
            ),
            Self::QueueFull { .. } => {
                Some("Raise pg_tviews.max_queue_size, or write in smaller transactions.".into())
            }
            Self::DepthExceeded { what, .. } if *what == "dependency" => Some(
                "Raise pg_tviews.max_dependency_depth, or flatten the views the TVIEW reads."
                    .into(),
            ),
            Self::CatalogError { .. } | Self::SerializationError { .. } => {
                Some("tviews.pg_tviews_reregister(name) re-derives a TVIEW's metadata.".into())
            }
            _ => None,
        }
    }

    /// Raise this error as a `PostgreSQL` ERROR. Does not return.
    #[track_caller]
    pub fn raise(self) -> ! {
        ErrorReport::from(self).report(PgLogLevel::ERROR);
        unreachable!("an ERROR report does not return")
    }

    /// This error with `context` before its message, keeping its SQLSTATE, detail
    /// and hint.
    #[track_caller]
    #[must_use]
    pub fn report_in(self, context: &str) -> ErrorReport {
        let mut report =
            ErrorReport::new(self.errcode(), format!("{context}: {self}"), "pg_tviews");
        if let Some(detail) = self.detail() {
            report = report.set_detail(detail);
        }
        if let Some(hint) = self.hint() {
            report = report.set_hint(hint);
        }
        report
    }

    /// Raise this error with `context` before its message. Does not return.
    #[track_caller]
    pub fn raise_in(self, context: &str) -> ! {
        self.report_in(context).report(PgLogLevel::ERROR);
        unreachable!("an ERROR report does not return")
    }
}

/// The text form of a `PostgreSQL` error code: five characters of six bits each
/// (`MAKE_SQLSTATE`).
fn sqlstate_text(code: PgSqlErrorCode) -> String {
    let packed = code as isize;
    (0..5)
        .map(|i| {
            let six = u8::try_from((packed >> (6 * i)) & 0x3F).unwrap_or(0);
            char::from(six + b'0')
        })
        .collect()
}

impl From<TViewError> for ErrorReport {
    #[track_caller]
    fn from(e: TViewError) -> Self {
        let mut report = Self::new(e.errcode(), e.to_string(), "pg_tviews");
        if let Some(detail) = e.detail() {
            report = report.set_detail(detail);
        }
        if let Some(hint) = e.hint() {
            report = report.set_hint(hint);
        }
        report
    }
}

impl fmt::Display for TViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MetadataNotFound { entity } => {
                write!(f, "TVIEW metadata not found for entity '{entity}'")
            }
            Self::RelationExists { name } => write!(f, "TVIEW {name} already exists"),
            Self::InvalidInput { parameter, reason } => {
                write!(f, "Invalid input for parameter '{parameter}': {reason}")
            }
            Self::DefinitionRefused { reason }
            | Self::PermissionDenied { reason }
            | Self::WrongState { reason } => {
                write!(f, "{reason}")
            }
            Self::KeyTypeRefused { column, found } => write!(
                f,
                "{column} is {found}: a TVIEW's pk_<entity> must be an integer key \
                 (smallint, integer or bigint)"
            ),
            Self::ColumnDdlRefused { table, change } => write!(
                f,
                "{change} on TVIEW {table} is refused: a TVIEW's columns are its definition's"
            ),
            Self::IndexNameReserved { table, index } => write!(
                f,
                "index name {index} on TVIEW {table} is reserved for pg_tviews' own index"
            ),
            Self::DependencyCycle { entities } => write!(
                f,
                "relations would read each other in a cycle: {}",
                entities.join(", ")
            ),
            Self::DepthExceeded {
                what,
                depth,
                max_depth,
            } => write!(f, "{what} depth {depth} exceeds the maximum of {max_depth}"),
            Self::InvalidSelectStatement { reason, .. } => {
                write!(f, "Invalid SELECT statement: {reason}")
            }
            Self::RequiredColumnMissing {
                column_name,
                context,
            } => write!(f, "Required column '{column_name}' missing in {context}"),
            Self::JsonbDeltaMissing => {
                write!(f, "Required extension 'jsonb_delta' is not installed")
            }
            Self::QueueFull { size, max_size } => write!(
                f,
                "refresh queue backpressure: queue size ({size}) would exceed \
                 max_queue_size ({max_size})"
            ),
            Self::CatalogError {
                operation,
                pg_error,
            } => write!(f, "Catalog operation '{operation}' failed: {pg_error}"),
            Self::SpiError { error, .. } => write!(f, "SPI query failed: {error}"),
            Self::SerializationError { message } => write!(f, "Serialization error: {message}"),
        }
    }
}

impl std::error::Error for TViewError {}

/// Result type for TVIEW operations
pub type TViewResult<T> = Result<T, TViewError>;

/// An SPI error from pgrx. The query is unknown here; callers that know it build
/// [`TViewError::SpiError`] themselves.
impl From<pgrx::spi::Error> for TViewError {
    fn from(e: pgrx::spi::Error) -> Self {
        Self::SpiError {
            query: String::new(),
            error: e.to_string(),
        }
    }
}

impl From<serde_json::Error> for TViewError {
    fn from(e: serde_json::Error) -> Self {
        Self::SerializationError {
            message: format!("JSON serialization error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of each variant: a new variant fails to compile here until it is listed.
    fn every_variant() -> Vec<TViewError> {
        let s = String::new;
        let all = vec![
            TViewError::MetadataNotFound { entity: s() },
            TViewError::RelationExists { name: s() },
            TViewError::InvalidInput {
                parameter: s(),
                reason: s(),
            },
            TViewError::DefinitionRefused { reason: s() },
            TViewError::KeyTypeRefused {
                column: s(),
                found: s(),
            },
            TViewError::ColumnDdlRefused {
                table: s(),
                change: s(),
            },
            TViewError::IndexNameReserved {
                table: s(),
                index: s(),
            },
            TViewError::PermissionDenied { reason: s() },
            TViewError::DependencyCycle { entities: vec![] },
            TViewError::DepthExceeded {
                what: "dependency",
                depth: 1,
                max_depth: 1,
            },
            TViewError::InvalidSelectStatement {
                sql: s(),
                reason: s(),
            },
            TViewError::RequiredColumnMissing {
                column_name: s(),
                context: s(),
            },
            TViewError::JsonbDeltaMissing,
            TViewError::QueueFull {
                size: 1,
                max_size: 1,
            },
            TViewError::WrongState { reason: s() },
            TViewError::CatalogError {
                operation: s(),
                pg_error: s(),
            },
            TViewError::SpiError {
                query: s(),
                error: s(),
            },
            TViewError::SerializationError { message: s() },
        ];
        for e in &all {
            // Exhaustive: adding a variant breaks this match until it is listed above.
            match e {
                TViewError::MetadataNotFound { .. }
                | TViewError::RelationExists { .. }
                | TViewError::InvalidInput { .. }
                | TViewError::DefinitionRefused { .. }
                | TViewError::KeyTypeRefused { .. }
                | TViewError::ColumnDdlRefused { .. }
                | TViewError::IndexNameReserved { .. }
                | TViewError::PermissionDenied { .. }
                | TViewError::DependencyCycle { .. }
                | TViewError::DepthExceeded { .. }
                | TViewError::InvalidSelectStatement { .. }
                | TViewError::RequiredColumnMissing { .. }
                | TViewError::JsonbDeltaMissing
                | TViewError::QueueFull { .. }
                | TViewError::WrongState { .. }
                | TViewError::CatalogError { .. }
                | TViewError::SpiError { .. }
                | TViewError::SerializationError { .. } => {}
            }
        }
        all
    }

    const fn is_internal(e: &TViewError) -> bool {
        matches!(
            e,
            TViewError::CatalogError { .. }
                | TViewError::SpiError { .. }
                | TViewError::SerializationError { .. }
        )
    }

    #[test]
    fn sqlstate_text_decodes_the_packed_code() {
        assert_eq!(
            sqlstate_text(PgSqlErrorCode::ERRCODE_UNDEFINED_OBJECT),
            "42704"
        );
        assert_eq!(
            sqlstate_text(PgSqlErrorCode::ERRCODE_INVALID_OBJECT_DEFINITION),
            "42P17"
        );
        assert_eq!(
            sqlstate_text(PgSqlErrorCode::ERRCODE_INTERNAL_ERROR),
            "XX000"
        );
    }

    #[test]
    fn each_user_facing_variant_has_its_own_sqlstate() {
        let all = every_variant();
        let user: Vec<String> = all
            .iter()
            .filter(|e| !is_internal(e))
            .map(TViewError::sqlstate)
            .collect();
        let unique: std::collections::HashSet<&String> = user.iter().collect();
        assert_eq!(unique.len(), user.len(), "shared SQLSTATEs: {user:?}");
        for e in all.iter().filter(|e| is_internal(e)) {
            assert_eq!(e.sqlstate(), "XX000", "{e:?}");
        }
    }

    #[test]
    fn messages_are_one_line() {
        let long = format!("SELECT {}é{}\nFROM t", "x".repeat(99), "y".repeat(300));
        for mut e in every_variant() {
            if let TViewError::SpiError { query, .. }
            | TViewError::InvalidSelectStatement { sql: query, .. } = &mut e
            {
                query.clone_from(&long);
            }
            let msg = e.to_string();
            assert!(!msg.contains('\n'), "{e:?}: {msg}");
        }
    }

    #[test]
    fn long_text_in_detail_is_cut_on_a_char_boundary() {
        // 'é' is two bytes; this puts it across byte 200.
        let query = format!("{}é{}", "x".repeat(199), "y".repeat(50));
        let err = TViewError::SpiError {
            query,
            error: "boom".to_string(),
        };
        assert!(err.to_string().contains("boom"));
        assert!(err.detail().is_some_and(|d| d.ends_with('x')));
    }

    #[test]
    fn documented_codes() {
        let code = |e: TViewError| e.sqlstate();
        assert_eq!(
            code(TViewError::MetadataNotFound {
                entity: "post".into()
            }),
            "42704"
        );
        assert_eq!(
            code(TViewError::RelationExists {
                name: "tv_post".into()
            }),
            "42P07"
        );
        assert_eq!(
            code(TViewError::DependencyCycle { entities: vec![] }),
            "42P17"
        );
        assert_eq!(
            code(TViewError::QueueFull {
                size: 1,
                max_size: 1
            }),
            "54000"
        );
        assert_eq!(
            code(TViewError::WrongState {
                reason: String::new()
            }),
            "55000"
        );
        assert_eq!(
            code(TViewError::InvalidSelectStatement {
                sql: String::new(),
                reason: String::new()
            }),
            "42601"
        );
        assert_eq!(
            code(TViewError::PermissionDenied {
                reason: String::new()
            }),
            "42501"
        );
    }
}

//! `docs/error-reference.md`, generated from [`TViewError`] and the errors raised
//! outside it, and the tests that keep it current.
//!
//! After changing an error, regenerate the page:
//!
//! ```text
//! UPDATE_ERROR_REFERENCE=1 cargo test --lib --no-default-features --features pg18 error_reference
//! ```

use super::{TViewError, sqlstate_text};
use pgrx::PgSqlErrorCode;
use std::fmt::Write as _;

const PAGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/error-reference.md");

/// The variant's name. Exhaustive: a new variant fails to compile here until it
/// is named, and then needs an example in [`examples`].
const fn variant_name(e: &TViewError) -> &'static str {
    match e {
        TViewError::MetadataNotFound { .. } => "MetadataNotFound",
        TViewError::RelationExists { .. } => "RelationExists",
        TViewError::InvalidInput { .. } => "InvalidInput",
        TViewError::DefinitionRefused { .. } => "DefinitionRefused",
        TViewError::KeyTypeRefused { .. } => "KeyTypeRefused",
        TViewError::ColumnDdlRefused { .. } => "ColumnDdlRefused",
        TViewError::PermissionDenied { .. } => "PermissionDenied",
        TViewError::DependencyCycle { .. } => "DependencyCycle",
        TViewError::DepthExceeded { .. } => "DepthExceeded",
        TViewError::InvalidSelectStatement { .. } => "InvalidSelectStatement",
        TViewError::RequiredColumnMissing { .. } => "RequiredColumnMissing",
        TViewError::JsonbDeltaMissing => "JsonbDeltaMissing",
        TViewError::QueueFull { .. } => "QueueFull",
        TViewError::WrongState { .. } => "WrongState",
        TViewError::CatalogError { .. } => "CatalogError",
        TViewError::SpiError { .. } => "SpiError",
        TViewError::SerializationError { .. } => "SerializationError",
    }
}

/// One of each variant, its values as placeholders.
fn examples() -> Vec<TViewError> {
    let p = |s: &str| format!("<{s}>");
    vec![
        TViewError::MetadataNotFound {
            entity: p("entity"),
        },
        TViewError::RelationExists { name: p("name") },
        TViewError::InvalidInput {
            parameter: p("parameter"),
            reason: p("reason"),
        },
        TViewError::DefinitionRefused {
            reason: p("reason"),
        },
        TViewError::KeyTypeRefused {
            column: p("column"),
            found: p("type"),
        },
        TViewError::ColumnDdlRefused {
            table: p("table"),
            change: p("statement"),
        },
        TViewError::PermissionDenied {
            reason: p("reason"),
        },
        TViewError::DependencyCycle {
            entities: vec![p("relation"), p("relation")],
        },
        TViewError::DepthExceeded {
            what: "dependency",
            depth: 11,
            max_depth: 10,
        },
        TViewError::InvalidSelectStatement {
            sql: p("definition"),
            reason: p("reason"),
        },
        TViewError::RequiredColumnMissing {
            column_name: p("column"),
            context: p("context"),
        },
        TViewError::JsonbDeltaMissing,
        TViewError::QueueFull {
            size: 10_001,
            max_size: 10_000,
        },
        TViewError::WrongState {
            reason: p("reason"),
        },
        TViewError::CatalogError {
            operation: p("operation"),
            pg_error: p("error"),
        },
        TViewError::SpiError {
            query: p("query"),
            error: p("error"),
        },
        TViewError::SerializationError {
            message: p("message"),
        },
    ]
}

/// The errors raised with a SQLSTATE of their own outside [`TViewError`]:
/// (source file, condition, when). A test checks every such site is listed.
const RAISED_ELSEWHERE: &[(&str, PgSqlErrorCode, &str)] = &[
    (
        "src/concurrency/mod.rs",
        PgSqlErrorCode::ERRCODE_T_R_SERIALIZATION_FAILURE,
        "Under `REPEATABLE READ` (ADR 0207): a write or a refresh needs a value lock a \
         concurrent transaction holds (`a concurrent transaction changes rows this TVIEW \
         refresh reads`, `… refreshes TVIEW rows from rows this write changes`), since waiting \
         could not make the snapshot see the other's change; or the latest snapshot shows \
         what the transaction's missed (`a TVIEW row this transaction refreshed changed in a \
         concurrent transaction`, `a concurrent transaction added rows this write must \
         refresh`). Retry the transaction.",
    ),
    (
        "src/flush/xact.rs",
        PgSqlErrorCode::ERRCODE_OBJECT_NOT_IN_PREREQUISITE_STATE,
        "A transaction commits with refresh work still queued (a missing or disabled flush \
         trigger): the commit fails.",
    ),
    (
        "src/revision.rs",
        PgSqlErrorCode::ERRCODE_OBJECT_NOT_IN_PREREQUISITE_STATE,
        "The library's catalog revision does not match the installed extension, or cannot be \
         read: run `ALTER EXTENSION pg_tviews UPDATE` (the hint names the fix).",
    ),
    (
        "src/owner.rs",
        PgSqlErrorCode::ERRCODE_INSUFFICIENT_PRIVILEGE,
        "The caller neither owns the TVIEW (or is a member of its owner) nor owns the \
         extension.",
    ),
    (
        "src/hooks/mod.rs",
        PgSqlErrorCode::ERRCODE_FEATURE_NOT_SUPPORTED,
        "A `CREATE TABLE tv_* AS` form pg_tviews cannot make a TVIEW of: `SELECT … INTO`, \
         `EXPLAIN`, `EXECUTE`, `WITH NO DATA`, a temporary table, a column list, \
         `TABLESPACE`, `USING`, a storage parameter other than `fillfactor`, a query with \
         parameters.",
    ),
    (
        "src/hooks/ctas.rs",
        PgSqlErrorCode::ERRCODE_DUPLICATE_TABLE,
        "`CREATE TABLE tv_* AS` names a TVIEW that already exists.",
    ),
    (
        "src/refresh/row.rs",
        PgSqlErrorCode::ERRCODE_CARDINALITY_VIOLATION,
        "A UNION backing view returns two rows for one key \
         (`pg_tviews.union_duplicate_policy = 'error'`).",
    ),
    (
        "src/ddl/uncascaded.rs",
        PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
        "Under `uncascaded_policy = 'error'`: a table whose writes no cascade maps to the \
         TVIEW's keys, a function that reads tables not declared in `function_reads`, or a \
         definition reading the time without `time_refresh`. Under `warn` the same is a \
         WARNING (01000).",
    ),
];

/// `undefined_object` for `ERRCODE_UNDEFINED_OBJECT`.
fn condition(code: PgSqlErrorCode) -> String {
    format!("{code:?}")
        .trim_start_matches("ERRCODE_")
        .to_lowercase()
}

/// `text` in a Markdown table cell: pipes escaped, and `<placeholder>`s kept as
/// text rather than read as HTML.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
        .replace('\n', " ")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The page.
fn render() -> String {
    let mut page = String::from(
        "# Error reference\n\
         \n\
         Every error pg_tviews raises carries a SQLSTATE a client can catch\n\
         (`EXCEPTION WHEN undefined_object`, `WHEN sqlstate '42704'`), a one-line\n\
         message, and where it helps a DETAIL (the definition or query involved) and a\n\
         HINT (what to do). Internal errors keep `XX000`.\n\
         \n\
         This page is generated from `src/error/` (`src/error/reference.rs`); a unit test\n\
         fails when it is out of date.\n\
         \n\
         ## Errors of pg_tviews functions\n\
         \n\
         Placeholders stand for the values each message carries.\n\
         \n\
         | SQLSTATE | Condition | Error | Message | Hint |\n\
         |---|---|---|---|---|\n",
    );
    for e in examples() {
        let _ = writeln!(
            page,
            "| `{}` | `{}` | `{}` | {} | {} |",
            e.sqlstate(),
            condition(e.errcode()),
            variant_name(&e),
            cell(&e.to_string()),
            e.hint().map_or_else(String::new, |h| cell(&h)),
        );
    }
    page.push_str(
        "\n\
         ## Errors raised by triggers, hooks and the commit\n\
         \n\
         | SQLSTATE | Condition | When |\n\
         |---|---|---|\n",
    );
    for (_, code, when) in RAISED_ELSEWHERE {
        let _ = writeln!(
            page,
            "| `{}` | `{}` | {} |",
            sqlstate_text(*code),
            condition(*code),
            cell(when),
        );
    }
    page
}

#[test]
fn every_variant_has_one_example() {
    let names: Vec<&str> = examples().iter().map(variant_name).collect();
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        names.len(),
        unique.len(),
        "an example listed twice: {names:?}"
    );
    assert_eq!(names.len(), 17, "a variant has no example: {names:?}");
}

/// Every SQLSTATE raised in place outside `src/error/` is in [`RAISED_ELSEWHERE`].
#[test]
fn every_sqlstate_raised_elsewhere_is_documented() {
    fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    for file in files {
        let rel = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if rel.starts_with("src/error/") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (i, _) in text.match_indices("PgSqlErrorCode::ERRCODE_") {
            let name: String = text[i + "PgSqlErrorCode::".len()..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || *c == '_')
                .collect();
            if name == "ERRCODE_WARNING" {
                continue;
            }
            assert!(
                RAISED_ELSEWHERE
                    .iter()
                    .any(|(f, code, _)| *f == rel && format!("{code:?}") == name),
                "{rel} raises {name}: list it in RAISED_ELSEWHERE"
            );
        }
    }
}

#[test]
fn error_reference_is_current() {
    let page = render();
    if std::env::var_os("UPDATE_ERROR_REFERENCE").is_some() {
        std::fs::write(PAGE, &page).unwrap();
        return;
    }
    let current = std::fs::read_to_string(PAGE).unwrap_or_default();
    assert!(
        current == page,
        "docs/error-reference.md is out of date: regenerate it with \
         UPDATE_ERROR_REFERENCE=1 cargo test --lib --no-default-features --features pg18 \
         error_reference"
    );
}

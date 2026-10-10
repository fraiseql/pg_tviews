//! Whether the `jsonb_delta` extension is installed, and in which schema: the
//! smart patches call its functions.

use pgrx::prelude::*;

/// [`crate::utils::log_once`] key of "`jsonb_delta` is not installed".
pub const JSONB_DELTA_MISSING: &str = "jsonb_delta_missing";

/// Check if `jsonb_delta` extension is available at runtime (cached)
/// Returns true if extension is installed, false otherwise
///
/// This function caches the result after the first check to avoid
/// repeated queries to `pg_extension` on every cascade operation.
#[must_use]
pub fn check_jsonb_delta_available() -> bool {
    jsonb_delta_schema().is_some()
}

/// Quoted schema of the `jsonb_delta` extension, for a patch about to be applied.
///
/// # Errors
/// [`crate::TViewError::JsonbDeltaMissing`] when it is not installed (dropped
/// since the patch was captured): an unqualified or `public` fallback would call
/// whatever function of that name a role with CREATE there planted.
pub fn require_jsonb_delta_schema() -> crate::TViewResult<String> {
    jsonb_delta_schema().ok_or(crate::TViewError::JsonbDeltaMissing)
}

/// Quoted schema of the `jsonb_delta` extension (cached), `None` when it is not
/// installed. Patch calls are qualified with it so they do not depend on the
/// session's `search_path`.
pub fn jsonb_delta_schema() -> Option<String> {
    if let Some(schema) = crate::cache::JSONB_DELTA_SCHEMA.with(|m| m.get(&())) {
        return schema;
    }
    let schema = Spi::connect(|client| {
        client
            .select(
                "SELECT pg_catalog.quote_ident(n.nspname) \
                 FROM pg_catalog.pg_extension e \
                 JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace \
                 WHERE e.extname = 'jsonb_delta'",
                None,
                &[],
            )?
            .first()
            .get_one::<String>()
    })
    .ok()
    .flatten();
    crate::cache::JSONB_DELTA_SCHEMA.with(|m| m.insert((), schema.clone()));
    schema
}

/// SQL function: whether `jsonb_delta` is installed (cached).
#[pg_extern]
fn pg_tviews_check_jsonb_delta() -> bool {
    check_jsonb_delta_available()
}

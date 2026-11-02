//! The schema of the `jsonb_delta` extension, which `pg_tviews` requires
//! (`requires` in its control file): the patches call its functions qualified
//! with it, so they do not depend on the session's `search_path`.

thread_local! {
    /// The quoted schema of `jsonb_delta`.
    static JSONB_DELTA_SCHEMA: crate::cache::Memo<(), String> = crate::cache::Memo::new();
}

/// Quoted schema of the `jsonb_delta` extension, read once per backend.
///
/// # Errors
/// A [`crate::TViewError::CatalogError`] when it cannot be read, or when the
/// extension is missing (it cannot be dropped while `pg_tviews` is installed).
pub fn schema() -> crate::TViewResult<String> {
    if let Some(schema) = JSONB_DELTA_SCHEMA.with(|m| m.get(&())) {
        return Ok(schema);
    }
    let schema = crate::utils::spi::catalog_one::<String>(
        "SELECT pg_catalog.quote_ident(n.nspname) \
         FROM pg_catalog.pg_extension e \
         JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace \
         WHERE e.extname = 'jsonb_delta'",
        &[],
    )?
    .ok_or_else(|| crate::TViewError::CatalogError {
        operation: "Find the jsonb_delta extension".to_string(),
        pg_error: "it is not installed, and pg_tviews requires it".to_string(),
    })?;
    JSONB_DELTA_SCHEMA.with(|m| m.insert((), schema.clone()));
    Ok(schema)
}

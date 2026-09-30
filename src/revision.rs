//! Library/catalog revision guard (issue #137).
//!
//! The library and the installed extension SQL each carry a catalog revision: the
//! library as [`CATALOG_REVISION`], the catalog as the SQL function
//! `tviews.pg_tviews_catalog_revision()`, which every upgrade script that changes
//! the extension SQL redefines. Before `pg_tviews` does real work in a backend it
//! checks that the two agree, so a library installed without
//! `ALTER EXTENSION pg_tviews UPDATE` fails with that command as the hint instead
//! of running against a catalog it does not know.

use pgrx::pg_sys;
use pgrx::prelude::*;
use std::cell::Cell;

/// Revision of the catalog this library works with. Bumped by a change to the
/// extension SQL, together with the upgrade script that redefines
/// `pg_tviews_catalog_revision()`.
pub const CATALOG_REVISION: i32 = 1;

thread_local! {
    /// Whether this backend already found a matching catalog. A mismatch is not
    /// cached, so the first call after `ALTER EXTENSION … UPDATE` passes.
    static MATCHED: Cell<bool> = const { Cell::new(false) };
}

/// What the installed catalog says about its revision.
pub enum Installed {
    Matches,
    /// Another revision.
    Differs(i32),
    /// A catalog without `pg_tviews_catalog_revision()`: a `0.1.0` install.
    Unversioned,
}

/// Raise an error unless the installed catalog has the library's revision.
///
/// Skipped while the extension's own script runs: inside a chained
/// `ALTER EXTENSION UPDATE` the intermediate revisions legitimately differ.
pub fn check() {
    if MATCHED.get() || in_own_script() {
        return;
    }
    match installed() {
        Installed::Matches => {}
        Installed::Differs(revision) => pg_sys::panic::ErrorReport::new(
            PgSqlErrorCode::ERRCODE_OBJECT_NOT_IN_PREREQUISITE_STATE,
            format!(
                "pg_tviews library catalog revision {CATALOG_REVISION} does not match the \
                 installed extension ({revision})"
            ),
            function_name!(),
        )
        .set_hint(remedy(revision))
        .report(PgLogLevel::ERROR),
        Installed::Unversioned => pg_sys::panic::ErrorReport::new(
            PgSqlErrorCode::ERRCODE_OBJECT_NOT_IN_PREREQUISITE_STATE,
            format!(
                "pg_tviews library catalog revision {CATALOG_REVISION} does not match the \
                 installed extension (0.1.0, no revision)"
            ),
            function_name!(),
        )
        .set_hint(
            "a 0.1.0 install cannot be updated in place: run scripts/migrate-from-0.1.0.sql \
             from the pg_tviews release",
        )
        .report(PgLogLevel::ERROR),
    }
}

/// What fixes a catalog of `revision` for this library.
#[must_use]
pub fn remedy(revision: i32) -> &'static str {
    if revision > CATALOG_REVISION {
        "the installed extension is newer than this library: install the pg_tviews package \
         that matches it"
    } else {
        "run ALTER EXTENSION pg_tviews UPDATE"
    }
}

/// Forget a match, so the next check compares again: after an abort (which may
/// have rolled back an `ALTER EXTENSION pg_tviews UPDATE`) and after DDL on the
/// extension itself.
pub fn reset() {
    MATCHED.set(false);
}

/// Compare the installed catalog's revision with the library's, without raising.
/// A match is remembered for the rest of the backend.
pub fn installed() -> Installed {
    if MATCHED.get() {
        return Installed::Matches;
    }
    // The function is looked up in the extension's own schema, so a 0.1.0 install
    // (another schema, no such function) is told apart from a wrong revision.
    let revision = Spi::connect(|client| {
        let schema = client
            .select(
                "SELECT pg_catalog.quote_ident(n.nspname) \
                 FROM pg_catalog.pg_extension e \
                 JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace \
                 JOIN pg_catalog.pg_proc p ON p.pronamespace = e.extnamespace \
                  AND p.proname = 'pg_tviews_catalog_revision' AND p.pronargs = 0 \
                 WHERE e.extname = 'pg_tviews'",
                None,
                &[],
            )?
            .first()
            .get_one::<String>()?;
        match schema {
            Some(schema) => client
                .select(
                    &format!("SELECT {schema}.pg_tviews_catalog_revision()"),
                    None,
                    &[],
                )?
                .first()
                .get_one::<i32>(),
            None => Ok(None),
        }
    })
    .ok()
    .flatten();
    match revision {
        Some(CATALOG_REVISION) => {
            MATCHED.set(true);
            Installed::Matches
        }
        Some(other) => Installed::Differs(other),
        None => Installed::Unversioned,
    }
}

/// Whether the `pg_tviews` install or upgrade script is running.
fn in_own_script() -> bool {
    // SAFETY: plain backend globals, read on the backend's own thread.
    unsafe {
        pg_sys::creating_extension
            && pg_sys::CurrentExtensionObject
                == pg_sys::get_extension_oid(c"pg_tviews".as_ptr(), true)
    }
}

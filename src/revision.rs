//! Library/catalog revision guard.
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
pub const CATALOG_REVISION: i32 = 10;

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
    /// The revision could not be read: why.
    Unreadable(String),
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
        Installed::Unreadable(why) => pg_sys::panic::ErrorReport::new(
            PgSqlErrorCode::ERRCODE_OBJECT_NOT_IN_PREREQUISITE_STATE,
            format!("pg_tviews could not read the installed catalog revision: {why}"),
            function_name!(),
        )
        .report(PgLogLevel::ERROR),
    }
}

/// Whether [`check`] passes, without raising.
pub fn is_current() -> bool {
    MATCHED.get() || in_own_script() || matches!(installed(), Installed::Matches)
}

/// What fixes a catalog of `revision` for this library.
#[must_use]
pub const fn remedy(revision: i32) -> &'static str {
    if revision > CATALOG_REVISION {
        "the installed extension is newer than this library: install the pg_tviews package \
         that matches it"
    } else {
        "run ALTER EXTENSION pg_tviews UPDATE"
    }
}

/// Forget a match, so the next check compares again: after an abort that ran DDL
/// (it may have rolled back an `ALTER EXTENSION pg_tviews UPDATE`) and after DDL
/// on the extension itself.
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
    let revision = revision_schema().map(|schema| {
        crate::utils::spi::catalog_one::<i32>(
            &format!("SELECT {schema}.pg_tviews_catalog_revision()"),
            &[],
        )
        .map(|revision| revision.ok_or("the revision function returned NULL"))
    });
    match revision.transpose() {
        Ok(Some(Ok(CATALOG_REVISION))) => {
            MATCHED.set(true);
            Installed::Matches
        }
        Ok(Some(Ok(other))) => Installed::Differs(other),
        Ok(Some(Err(why))) => Installed::Unreadable(why.to_string()),
        Ok(None) => Installed::Unversioned,
        Err(e) => Installed::Unreadable(e.to_string()),
    }
}

/// The quoted schema of the `pg_tviews` extension when the revision function is
/// in it, read from the system caches: no query to parse and plan in a new backend.
fn revision_schema() -> Option<String> {
    // SAFETY: catalog lookups by name and OID inside a transaction; the names are
    // NUL-terminated, the argument vector is built for the lookup.
    unsafe {
        let extension = pg_sys::get_extension_oid(c"pg_tviews".as_ptr(), true);
        if extension == pg_sys::InvalidOid {
            return None;
        }
        let schema = pg_sys::get_extension_schema(extension);
        let no_arguments = pg_sys::buildoidvector(std::ptr::null(), 0);
        let defined = pg_sys::SearchSysCacheExists(
            pg_sys::SysCacheIdentifier::PROCNAMEARGSNSP.cast_signed(),
            pg_sys::Datum::from(c"pg_tviews_catalog_revision".as_ptr()),
            pg_sys::Datum::from(no_arguments),
            pg_sys::Datum::from(schema),
            pg_sys::Datum::from(0),
        );
        let name = pg_sys::get_namespace_name(schema);
        if !defined || name.is_null() {
            return None;
        }
        let quoted = pg_sys::quote_identifier(name);
        Some(
            std::ffi::CStr::from_ptr(quoted)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// Whether the `pg_tviews` install or upgrade script is running: the catalog may be
/// one of an earlier release, between two upgrade scripts.
pub(crate) fn in_own_script() -> bool {
    // SAFETY: plain backend globals, read on the backend's own thread.
    unsafe {
        pg_sys::creating_extension
            && pg_sys::CurrentExtensionObject
                == pg_sys::get_extension_oid(c"pg_tviews".as_ptr(), true)
    }
}

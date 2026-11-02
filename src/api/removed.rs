//! The C symbols of functions a released catalog binds but this library no longer
//! provides. Only a catalog that `ALTER EXTENSION pg_tviews UPDATE` has not reached
//! yet calls them: each raises the revision error, which names the update, instead
//! of PostgreSQL's "could not find function" on the library.

use pgrx::pg_sys;
use pgrx::prelude::*;

/// The V1 calling convention record PostgreSQL asks each symbol for.
static V1: pg_sys::Pg_finfo_record = pg_sys::Pg_finfo_record { api_version: 1 };

/// Raise the error for a call of `function`, removed in 0.1.0-beta.35.
fn removed(function: &str) -> ! {
    crate::revision::check();
    pg_sys::panic::ErrorReport::new(
        PgSqlErrorCode::ERRCODE_UNDEFINED_FUNCTION,
        format!("{function}() was removed in pg_tviews 0.1.0-beta.35"),
        function_name!(),
    )
    .set_hint("Run ALTER EXTENSION pg_tviews UPDATE.")
    .report(PgLogLevel::ERROR);
    unreachable!("ERROR does not return")
}

macro_rules! removed_symbol {
    ($symbol:ident, $finfo:ident, $function:literal) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $finfo() -> &'static pg_sys::Pg_finfo_record {
            &V1
        }

        #[pg_guard]
        #[unsafe(no_mangle)]
        pub extern "C-unwind" fn $symbol(_fcinfo: pg_sys::FunctionCallInfo) -> pg_sys::Datum {
            removed($function)
        }
    };
}

// jsonb_delta is required: nothing checks for it.
removed_symbol!(
    pg_tviews_check_jsonb_delta_wrapper,
    pg_finfo_pg_tviews_check_jsonb_delta_wrapper,
    "pg_tviews_check_jsonb_delta"
);
// Folded into pg_tviews_show_queue().
removed_symbol!(
    pg_tviews_debug_queue_wrapper,
    pg_finfo_pg_tviews_debug_queue_wrapper,
    "pg_tviews_debug_queue"
);
// Folded into pg_tviews_show_suspended().
removed_symbol!(
    pg_tviews_is_suspended_wrapper,
    pg_finfo_pg_tviews_is_suspended_wrapper,
    "pg_tviews_is_suspended"
);

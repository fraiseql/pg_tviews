//! Startup worker that repopulates UNLOGGED TVIEWs once recovery has finished.
//!
//! Promotion and crash recovery reset UNLOGGED tables to their empty init fork.
//! Without this worker such a TVIEW stays empty for readers until a write
//! touches it. For every database listed in `pg_tviews.auto_rebuild_databases`
//! a worker starts when the server leaves recovery (at startup, after a crash
//! restart or on promotion; never on a standby), calls
//! [`crate::replication::rebuild_all`] and then idles, so a later crash restart
//! runs it again.

use pgrx::bgworkers::{
    BackgroundWorker, BackgroundWorkerBuilder, BgWorkerStartTime, SignalWakeFlags,
};
use pgrx::prelude::*;
use std::time::Duration;

/// Register one worker per configured database. Must run from `_PG_init` while
/// `shared_preload_libraries` is being processed.
pub fn register() {
    // SAFETY: reads a postmaster-owned global set before libraries are preloaded.
    if !unsafe { pg_sys::process_shared_preload_libraries_in_progress } {
        return;
    }
    for database in crate::config::auto_rebuild_databases() {
        BackgroundWorkerBuilder::new(&format!("pg_tviews rebuild {database}"))
            .set_type("pg_tviews rebuild")
            .set_library("pg_tviews")
            .set_function("pg_tviews_rebuild_worker_main")
            .set_extra(&database)
            .enable_spi_access()
            .set_start_time(BgWorkerStartTime::RecoveryFinished)
            .set_restart_time(Some(Duration::from_secs(60)))
            .load();
    }
}

/// Worker entry point: rebuild the emptied TVIEWs of one database, then idle.
#[pg_guard]
#[unsafe(no_mangle)]
pub extern "C-unwind" fn pg_tviews_rebuild_worker_main(_arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);
    let database = BackgroundWorker::get_extra().to_string();
    BackgroundWorker::connect_worker_to_spi(Some(&database), None);

    BackgroundWorker::transaction(|| {
        let schema = Spi::get_one::<String>(
            "SELECT n.nspname::text FROM pg_extension e \
             JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'pg_tviews'",
        )
        .ok()
        .flatten();
        let Some(schema) = schema else {
            log!(
                "pg_tviews: extension not installed in database \"{database}\"; nothing to rebuild"
            );
            return;
        };
        let path = format!("{}, public", crate::utils::quote_identifier(&schema));
        if let Err(e) = Spi::run(&format!(
            "SELECT pg_catalog.set_config('search_path', {}, true)",
            crate::utils::quote_literal(&path)
        )) {
            warning!("pg_tviews: could not set search_path in database \"{database}\": {e}");
            return;
        }
        match crate::replication::rebuild_all(true) {
            Ok(rebuilt) if rebuilt.is_empty() => {
                log!("pg_tviews: no TVIEW to rebuild in database \"{database}\"");
            }
            Ok(rebuilt) => {
                for (entity, rows) in rebuilt {
                    log!("pg_tviews: rebuilt tv_{entity} ({rows} rows) in database \"{database}\"");
                }
            }
            Err(e) => warning!("pg_tviews: rebuild in database \"{database}\" failed: {e}"),
        }
    });

    while BackgroundWorker::wait_latch(Some(Duration::from_secs(3600))) {}
}

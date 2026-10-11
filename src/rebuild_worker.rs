//! Background workers that refill reset UNLOGGED TVIEWs once recovery ends.
//!
//! Promotion and crash recovery reset UNLOGGED tables to their empty init fork.
//! Without these workers such a TVIEW stays empty for readers until a write
//! touches it. A launcher starts when the server leaves recovery (at startup,
//! after a crash restart or on promotion; never on a standby), lists the
//! databases `pg_tviews.auto_rebuild_databases` names (`*`, the default: every
//! database that accepts connections), and starts one worker per database, one
//! after the other. Each worker calls [`crate::replication::rebuild_all`] in its
//! database and exits; the launcher then idles, so that a later crash restart
//! runs it again.

use pgrx::bgworkers::{
    BackgroundWorker, BackgroundWorkerBuilder, BgWorkerStartTime, SignalWakeFlags,
};
use pgrx::prelude::*;
use std::time::Duration;

/// Register the launcher. Must run from `_PG_init` while
/// `shared_preload_libraries` is being processed.
pub fn register() {
    // SAFETY: reads a postmaster-owned global set before libraries are preloaded.
    if !unsafe { pg_sys::process_shared_preload_libraries_in_progress } {
        return;
    }
    if crate::config::rebuild_databases() == crate::config::RebuildDatabases::None {
        return;
    }
    BackgroundWorkerBuilder::new("pg_tviews rebuild launcher")
        .set_type("pg_tviews rebuild launcher")
        .set_library("pg_tviews")
        .set_function("pg_tviews_rebuild_launcher_main")
        .enable_spi_access()
        .set_start_time(BgWorkerStartTime::RecoveryFinished)
        .set_restart_time(Some(Duration::from_secs(60)))
        .load();
}

/// Launcher entry point: a worker per database to rebuild in, one at a time, then idle.
#[pg_guard]
#[unsafe(no_mangle)]
pub extern "C-unwind" fn pg_tviews_rebuild_launcher_main(_arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);
    // No database: pg_database is a shared catalog.
    BackgroundWorker::connect_worker_to_spi(None, None);
    let databases = BackgroundWorker::transaction(databases_accepting_connections);
    let chosen = match crate::config::rebuild_databases() {
        crate::config::RebuildDatabases::All => databases,
        crate::config::RebuildDatabases::Only(listed) => {
            for missing in listed.iter().filter(|d| !databases.contains(d)) {
                log!(
                    "pg_tviews: database \"{missing}\" in pg_tviews.auto_rebuild_databases does not accept connections or does not exist"
                );
            }
            listed
                .into_iter()
                .filter(|d| databases.contains(d))
                .collect()
        }
        crate::config::RebuildDatabases::None => Vec::new(),
    };
    for database in chosen {
        if BackgroundWorker::sigterm_received() {
            return;
        }
        // SAFETY: reads a backend global.
        let me = unsafe { pg_sys::MyProcPid };
        let started = BackgroundWorkerBuilder::new(&format!("pg_tviews rebuild {database}"))
            .set_type("pg_tviews rebuild")
            .set_library("pg_tviews")
            .set_function("pg_tviews_rebuild_worker_main")
            .set_extra(&database)
            .enable_spi_access()
            .set_notify_pid(me)
            .load_dynamic();
        match started {
            Ok(worker) => {
                if let Err(status) = worker.wait_for_shutdown() {
                    log!(
                        "pg_tviews: the rebuild worker of database \"{database}\" ended: {status:?}"
                    );
                }
            }
            Err(e) => {
                log!(
                    "pg_tviews: could not start the rebuild worker of database \"{database}\": {e:?}"
                );
            }
        }
    }
    while BackgroundWorker::wait_latch(Some(Duration::from_secs(3600))) {}
}

/// The databases that accept connections, by name, read from the shared catalog
/// as the autovacuum launcher reads it (no database connection needed).
fn databases_accepting_connections() -> Vec<String> {
    let mut names = Vec::new();
    // SAFETY: inside a transaction (`BackgroundWorker::transaction`); the scan of
    // pg_database is opened, read and closed here, under AccessShareLock.
    unsafe {
        let rel = pg_sys::table_open(
            pg_sys::DatabaseRelationId,
            pg_sys::AccessShareLock.cast_signed(),
        );
        let scan = pg_sys::table_beginscan_catalog(rel, 0, std::ptr::null_mut());
        loop {
            let tuple = pg_sys::heap_getnext(scan, pg_sys::ScanDirection::ForwardScanDirection);
            if tuple.is_null() {
                break;
            }
            let row = &*pg_sys::heap_tuple_get_struct::<pg_sys::FormData_pg_database>(tuple);
            if row.datallowconn && !row.datistemplate {
                names.push(
                    std::ffi::CStr::from_ptr(row.datname.data.as_ptr())
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        if let Some(end) = (*(*scan).rs_rd)
            .rd_tableam
            .as_ref()
            .and_then(|am| am.scan_end)
        {
            end(scan);
        }
        pg_sys::table_close(rel, pg_sys::AccessShareLock.cast_signed());
    }
    names.sort();
    names
}

/// Worker entry point: rebuild the emptied TVIEWs of one database, then exit.
#[pg_guard]
#[unsafe(no_mangle)]
pub extern "C-unwind" fn pg_tviews_rebuild_worker_main(_arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);
    let database = BackgroundWorker::get_extra().to_string();
    BackgroundWorker::connect_worker_to_spi(Some(&database), None);

    BackgroundWorker::transaction(|| {
        // An SPI error here is not "not installed": it ends the worker, and the
        // first write to a reset TVIEW refills it.
        let schema = Spi::get_one::<String>(
            "SELECT n.nspname::text FROM pg_extension e \
             JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'pg_tviews'",
        )
        .or_else(|e| match e {
            // No row: the extension is not installed here.
            pgrx::spi::Error::InvalidPosition => Ok(None),
            e => Err(e),
        })
        .unwrap_or_else(|e| {
            error!("pg_tviews: could not look up the extension in database \"{database}\": {e}")
        });
        let Some(schema) = schema else {
            log!(
                "pg_tviews: extension not installed in database \"{database}\"; nothing to rebuild"
            );
            return;
        };
        // A library installed without ALTER EXTENSION UPDATE: say so once and idle
        // until the next start, instead of failing into the restart loop.
        let remedy = match crate::revision::installed() {
            crate::revision::Installed::Matches => None,
            crate::revision::Installed::Differs(revision) => {
                Some(crate::revision::remedy(revision).to_string())
            }
            crate::revision::Installed::Unversioned => {
                Some("run scripts/migrate-from-0.1.0.sql from the pg_tviews release".to_string())
            }
            crate::revision::Installed::Unreadable(why) => {
                error!(
                    "pg_tviews: could not read the catalog revision in database \"{database}\": {why}"
                )
            }
        };
        if let Some(remedy) = remedy {
            log!(
                "pg_tviews: library catalog revision {} does not match the extension in \
                 database \"{database}\"; not rebuilding ({remedy})",
                crate::revision::CATALOG_REVISION
            );
            return;
        }
        let path = format!("{}, public", crate::utils::ident::quoted(&schema));
        let args = [crate::utils::spi::text(path.as_str())];
        if let Err(e) = Spi::run_with_args(
            "SELECT pg_catalog.set_config('search_path', $1, true)",
            &args,
        ) {
            error!("pg_tviews: could not set search_path in database \"{database}\": {e}");
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
            // Logged with the error; the first write to a reset TVIEW refills it.
            Err(e) => error!("pg_tviews: rebuild in database \"{database}\" failed: {e}"),
        }
    });
}

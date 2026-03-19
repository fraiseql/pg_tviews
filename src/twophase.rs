//! Two-phase commit (2PC) support: PREPARE, COMMIT PREPARED, ROLLBACK PREPARED.

use pgrx::prelude::*;
use pgrx::datum::DatumWithOid;
use pgrx::JsonB;
use crate::{TViewError, TViewResult};

/// Validate a 2PC global transaction ID (GID) for safe use in SQL.
///
/// Rejects empty strings, overly long values (`PostgreSQL` limit is 200 bytes),
/// and characters that could be used for SQL injection.
fn validate_gid(gid: &str) -> TViewResult<()> {
    if gid.is_empty() || gid.len() > 199 {
        return Err(TViewError::InvalidInput {
            parameter: "gid".to_string(),
            reason: format!("GID must be 1\u{2013}199 bytes (got {})", gid.len()),
        });
    }
    if gid.contains('\'') || gid.contains('\0') || gid.contains(';') {
        return Err(TViewError::InvalidInput {
            parameter: "gid".to_string(),
            reason: "GID contains invalid characters (', ;, or null byte)".to_string(),
        });
    }
    Ok(())
}

/// Handle COMMIT PREPARED for 2PC transactions
/// Processes pending refreshes for a committed prepared transaction
///
/// Arguments:
/// - `gid`: Global transaction ID of the prepared transaction
#[pg_extern]
fn pg_tviews_commit_prepared(gid: &str) -> TViewResult<()> {
    validate_gid(gid)?;

    // STEP 1: Load queue metadata BEFORE committing (verify it exists)
    let args = vec![unsafe { DatumWithOid::new(gid, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }];
    let queue_jsonb: Option<JsonB> = Spi::get_one_with_args(
        "SELECT refresh_queue FROM pg_tview_pending_refreshes WHERE gid = $1",
        &args,
    )?;

    // STEP 2: COMMIT THE PREPARED TRANSACTION FIRST
    // This ensures TVIEWs never show uncommitted data
    let commit_sql = format!("COMMIT PREPARED '{gid}'");
    Spi::run(&commit_sql)?;

    // STEP 3: Now process the queue (transaction is committed, safe to refresh)
    let Some(jsonb) = queue_jsonb else {
        return Ok(());
    };

    let serialized = crate::queue::persistence::SerializedQueue::from_jsonb(jsonb)?;
    let queue = serialized.into_queue();

    if !queue.is_empty() {
        Spi::run("BEGIN")?;

        match process_refresh_queue(queue) {
            Ok(()) => {
                Spi::run("COMMIT")?;
            }
            Err(e) => {
                Spi::run("ROLLBACK")?;
                return Err(e);
            }
        }
    }

    // STEP 4: Clean up persistent entry
    Spi::run_with_args(
        "DELETE FROM pg_tview_pending_refreshes WHERE gid = $1",
        &[unsafe { DatumWithOid::new(gid, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }],
    )?;

    Ok(())
}

/// Handle ROLLBACK PREPARED for 2PC transactions
/// Cleans up pending refreshes for a rolled back prepared transaction
///
/// Arguments:
/// - `gid`: Global transaction ID of the prepared transaction
#[pg_extern]
fn pg_tviews_rollback_prepared(gid: &str) -> TViewResult<()> {
    validate_gid(gid)?;

    let rollback_sql = format!("ROLLBACK PREPARED '{gid}'");
    Spi::run(&rollback_sql)?;

    Spi::get_one_with_args::<i32>(
        "DELETE FROM pg_tview_pending_refreshes WHERE gid = $1 RETURNING 1",
        &[unsafe { DatumWithOid::new(gid, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }],
    )?;

    Ok(())
}

/// Process refresh queue (extracted from `handle_pre_commit` for reuse)
fn process_refresh_queue(queue: std::collections::HashSet<crate::queue::RefreshKey>) -> TViewResult<()> {
    let mut pending = queue;
    let mut processed = std::collections::HashSet::new();
    let graph = crate::queue::cache::graph_cache::load_cached()?;

    let mut iteration = 1;
    while !pending.is_empty() {
        let sorted_keys = graph.sort_keys(pending.drain().collect());

        for key in sorted_keys {
            if !processed.insert(key.clone()) {
                continue;
            }

            let parents = refresh_and_get_parents(&key)?;

            for parent_key in parents {
                if !processed.contains(&parent_key) {
                    pending.insert(parent_key);
                }
            }
        }

        iteration += 1;
        if iteration > get_max_propagation_depth() {
            return Err(crate::TViewError::PropagationDepthExceeded {
                max_depth: get_max_propagation_depth(),
                processed: processed.len(),
            });
        }
    }

    Ok(())
}

/// Refresh a single entity+pk and return discovered parent keys
fn refresh_and_get_parents(key: &crate::queue::RefreshKey) -> TViewResult<Vec<crate::queue::RefreshKey>> {
    use crate::catalog::TviewMeta;
    let meta = TviewMeta::load_by_entity(&key.entity)?
        .ok_or_else(|| crate::TViewError::MetadataNotFound {
            entity: key.entity.clone(),
        })?;

    crate::refresh::refresh_pk(meta.view_oid, key.pk)?;

    crate::propagate::find_parents_for(key)
}

/// Get maximum propagation depth from config
fn get_max_propagation_depth() -> usize {
    crate::config::max_propagation_depth()
}

/// Recover orphaned prepared transactions
/// Processes pending refreshes for prepared transactions that may have been interrupted
///
/// Returns a table with recovery results: (gid, `queue_size`, status)
#[pg_extern]
fn pg_tviews_recover_prepared_transactions() -> pgrx::iter::TableIterator<
    'static,
    (
        pgrx::name!(gid, String),
        pgrx::name!(queue_size, i32),
        pgrx::name!(status, String),
    ),
> {
    let results: Vec<(String, i32, String)> = Spi::connect(|client| {
        const RECOVERY_LOCK_KEY: i64 = 0x7476_6965_7773_5F72; // "tviews_r" in hex

        let mut lock_result = client.select(
            &format!("SELECT pg_try_advisory_lock({RECOVERY_LOCK_KEY})"),
            None,
            &[],
        )?;

        let lock_acquired = if let Some(row) = lock_result.next() {
            row[1].value::<bool>()?.unwrap_or(false)
        } else {
            false
        };

        if !lock_acquired {
            return Ok(Vec::new());
        }

        let _guard = AdvisoryLockGuard::new(RECOVERY_LOCK_KEY);

        let rows = client.select(
            "SELECT gid, queue_size FROM pg_tview_pending_refreshes
             WHERE prepared_at < now() - interval '1 hour'
             ORDER BY prepared_at",
            None,
            &[],
        )?;

        let mut results = Vec::new();

        for row in rows {
            let gid: String = row["gid"].value()?
                .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                    query: "SELECT gid, queue_size FROM pg_tview_pending_refreshes ...".to_string(),
                    error: "gid column is NULL".to_string(),
                }))?;
            let queue_size: i32 = row["queue_size"].value()?
                .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                    query: "SELECT gid, queue_size FROM pg_tview_pending_refreshes ...".to_string(),
                    error: "queue_size column is NULL".to_string(),
                }))?;

            let status = match pg_tviews_commit_prepared(&gid) {
                Ok(()) => "processed".to_string(),
                Err(e) => {
                    warning!("TVIEW: Failed to recover prepared transaction '{}': {:?}", gid, e);
                    "error".to_string()
                }
            };

            results.push((gid, queue_size, status));
        }

        Ok::<_, spi::Error>(results)
    })
    .unwrap_or_else(|e| {
        warning!("Failed to list pending 2PC refreshes: {e:?}");
        Vec::new()
    });

    pgrx::iter::TableIterator::new(results)
}

/// RAII guard for advisory lock (ensures unlock on drop)
struct AdvisoryLockGuard {
    lock_key: i64,
}

impl AdvisoryLockGuard {
    const fn new(lock_key: i64) -> Self {
        Self { lock_key }
    }
}

impl Drop for AdvisoryLockGuard {
    fn drop(&mut self) {
        let _ = Spi::run(&format!("SELECT pg_advisory_unlock({})", self.lock_key));
    }
}

# Phase 8: Two-Phase Commit (2PC) Support & Advanced Features

**Status:** PLANNED (Post-Phase 7)
**Prerequisites:** Phase 6 Complete ✅, Phase 7 Optional
**Estimated Time:** 2-3 weeks
**Priority:** Low (Enterprise/distributed systems feature)

---

## Objective

Implement support for distributed transactions and advanced production features:

1. **Prepared Transaction Support**: Handle `PREPARE TRANSACTION` / `COMMIT PREPARED`
2. **Persistent Queue**: Serialize refresh queue to survive connection termination
3. **Automatic Recovery**: Resume pending refreshes after crash/restart
4. **Cross-Database Refresh**: Support TVIEWs spanning multiple databases
5. **Parallel Refresh**: Multi-worker refresh processing for large queues

---

## Context

### Why 2PC Support?

**Use Cases:**
- Distributed database systems (Citus, Postgres-XL)
- Multi-database transactions (dblink, postgres_fdw)
- XA-compliant applications (Java EE, enterprise apps)
- Saga pattern implementations
- Multi-tenant systems with database-per-tenant

**Current Limitation (Phase 6):**
```sql
-- Connection 1:
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- Queue: {("user", 1)} stored in thread-local storage

PREPARE TRANSACTION 'xact_42';
-- Transaction prepared, but queue is in thread-local storage
-- Connection 1 terminates

-- Queue is LOST (thread-local storage destroyed) ❌

-- Connection 2 (later):
COMMIT PREPARED 'xact_42';
-- ⚠️ Transaction commits WITHOUT refreshing TVIEWs
-- Result: tv_user and dependent views remain STALE
```

**Impact:**
- Silent data inconsistency
- TVIEWs out of sync with base tables
- No error raised (transaction commits successfully)

---

## Sub-Phases

Phase 8 is divided into 5 sub-tasks:

| Sub-Phase | Focus | Time | Dependencies |
|-----------|-------|------|--------------|
| **8A** | Persistent Queue Table | 2 days | Phase 6C ✅ |
| **8B** | PREPARE TRANSACTION Handling | 2-3 days | Phase 8A |
| **8C** | COMMIT PREPARED Handling | 2 days | Phase 8B |
| **8D** | Automatic Recovery | 2 days | Phase 8C |
| **8E** | Parallel Refresh | 3-4 days | Phase 6D ✅ |

---

## Phase 8A: Persistent Queue Table

### Schema Design

```sql
-- Table to persist queues for prepared transactions
CREATE TABLE pg_tview_pending_refreshes (
    gid TEXT PRIMARY KEY,  -- Global transaction ID
    refresh_queue JSONB NOT NULL,  -- Serialized queue: [{"entity": "user", "pk": 1}, ...]
    prepared_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    prepared_by TEXT,  -- Session user
    database_name TEXT NOT NULL DEFAULT current_database(),

    -- Metadata for monitoring
    queue_size INT NOT NULL,  -- Number of pending refreshes
    iteration_estimate INT,  -- Estimated propagation iterations

    -- Cleanup
    expires_at TIMESTAMPTZ,  -- Auto-cleanup after N hours

    CONSTRAINT pending_refreshes_gid_check CHECK (gid <> '')
);

CREATE INDEX ON pg_tview_pending_refreshes(prepared_at);
CREATE INDEX ON pg_tview_pending_refreshes(expires_at) WHERE expires_at IS NOT NULL;

-- Auto-cleanup function (called by cron or pg_cron)
CREATE FUNCTION pg_tviews_cleanup_expired_queues()
RETURNS INT AS $$
DELETE FROM pg_tview_pending_refreshes
WHERE expires_at < now()
RETURNING COUNT(*)::INT;
$$ LANGUAGE SQL;
```

### Serialization (JSONB vs Binary)

**Two options for queue storage:**

1. **JSONB** (easier to debug, human-readable)
2. **Binary** (3× more compact, faster for large queues)

**Recommendation:** Use **binary** for production (better performance), JSONB for development/debugging.

```rust
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializedQueue {
    version: u32,  // Schema version for forward compatibility
    keys: Vec<RefreshKey>,
    metadata: QueueMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueMetadata {
    enqueued_at: String,  // ISO8601 timestamp
    source_session: String,
    savepoint_depth: usize,
}

impl SerializedQueue {
    fn from_queue(queue: HashSet<RefreshKey>) -> Self {
        Self {
            version: 1,
            keys: queue.into_iter().collect(),
            metadata: QueueMetadata {
                enqueued_at: chrono::Utc::now().to_rfc3339(),
                source_session: get_session_id(),
                savepoint_depth: get_savepoint_depth(),
            },
        }
    }

    fn to_queue(self) -> HashSet<RefreshKey> {
        self.keys.into_iter().collect()
    }

    // OPTION 1: JSONB (human-readable, easier debugging)
    fn to_jsonb(self) -> JsonB {
        let json = serde_json::to_value(self).unwrap();
        JsonB(json)
    }

    fn from_jsonb(jsonb: JsonB) -> TViewResult<Self> {
        serde_json::from_value(jsonb.0)
            .map_err(|e| TViewError::SerializationError {
                message: format!("Failed to deserialize queue: {}", e),
            })
    }

    // OPTION 2: Binary (compact, faster for large queues)
    fn to_binary(&self) -> Vec<u8> {
        // Use bincode for efficient binary serialization
        bincode::serialize(self).unwrap()
    }

    fn from_binary(data: &[u8]) -> TViewResult<Self> {
        bincode::deserialize(data)
            .map_err(|e| TViewError::SerializationError {
                message: format!("Failed to deserialize binary queue: {}", e),
            })
    }

    // OPTION 3: Compressed JSONB (balance of readability and size)
    fn to_compressed_jsonb(&self) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let json = serde_json::to_vec(self).unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&json).unwrap();
        encoder.finish().unwrap()
    }

    fn from_compressed_jsonb(data: &[u8]) -> TViewResult<Self> {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut decoder = GzDecoder::new(data);
        let mut json_bytes = Vec::new();
        decoder.read_to_end(&mut json_bytes)
            .map_err(|e| TViewError::SerializationError {
                message: format!("Decompression failed: {}", e),
            })?;

        serde_json::from_slice(&json_bytes)
            .map_err(|e| TViewError::SerializationError {
                message: format!("Failed to deserialize JSON: {}", e),
            })
    }
}
```

**Size Comparison (10,000 keys):**
- JSONB: ~500 KB
- Binary (bincode): ~150 KB (3× smaller)
- Compressed JSONB (gzip): ~100 KB (5× smaller)

**Recommendation:** Use **binary** by default, make it configurable via GUC.

---

## Phase 8B: PREPARE TRANSACTION Handling

### Hook Into PREPARE

```rust
use once_cell::sync::Lazy;
use std::sync::Mutex;

// Global storage for GID during PREPARE TRANSACTION
static PREPARING_GID: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

/// ProcessUtility hook to capture GID during PREPARE TRANSACTION
static mut PREV_PROCESS_UTILITY: Option<pg_sys::ProcessUtility_hook_type> = None;

#[pg_guard]
unsafe extern "C" fn tview_process_utility_hook(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext,
    params: pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    completion_tag: *mut pg_sys::QueryCompletion,
) {
    // Check if this is PREPARE TRANSACTION
    let query_str = std::ffi::CStr::from_ptr(query_string).to_str().unwrap_or("");

    if query_str.trim().to_uppercase().starts_with("PREPARE TRANSACTION") {
        // Extract GID from query: PREPARE TRANSACTION 'gid'
        if let Some(gid) = extract_gid_from_prepare_query(query_str) {
            *PREPARING_GID.lock().unwrap() = Some(gid);
        }
    }

    // Call previous hook or standard ProcessUtility
    if let Some(prev_hook) = PREV_PROCESS_UTILITY {
        prev_hook(pstmt, query_string, read_only_tree, context, params,
                  query_env, dest, completion_tag);
    } else {
        pg_sys::standard_ProcessUtility(pstmt, query_string, read_only_tree,
                                       context, params, query_env, dest, completion_tag);
    }
}

fn extract_gid_from_prepare_query(query: &str) -> Option<String> {
    // Parse: PREPARE TRANSACTION 'gid' or PREPARE TRANSACTION "gid"
    let re = regex::Regex::new(r"PREPARE\s+TRANSACTION\s+['\"]([^'\"]+)['\"]").ok()?;
    re.captures(query)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

/// Called during PREPARE TRANSACTION
unsafe extern "C" fn tview_xact_callback(event: u32, _arg: *mut c_void) {
    let xact_event = match event {
        // Existing events
        0 => XactEvent::Commit,
        1 => XactEvent::PreCommit,
        2 => XactEvent::Abort,

        // New: PREPARE event
        4 => XactEvent::Prepare,  // XACT_EVENT_PREPARE
        _ => return,
    };

    match xact_event {
        XactEvent::Prepare => {
            // Serialize queue to pg_tview_pending_refreshes
            if let Err(e) = handle_prepare() {
                error!("TVIEW failed to persist queue during PREPARE: {:?}", e);
            }
        }
        // ... existing event handlers ...
    }
}

#[pg_guard]
pub extern "C" fn _PG_init() {
    // ... existing initialization ...

    // Install ProcessUtility hook to capture GID
    unsafe {
        PREV_PROCESS_UTILITY = pg_sys::ProcessUtility_hook;
        pg_sys::ProcessUtility_hook = Some(tview_process_utility_hook);
    }
}
```

### Implementation

```rust
fn handle_prepare() -> TViewResult<()> {
    // Get global transaction ID (GID)
    let gid = get_prepared_transaction_id()?;

    // Take snapshot of current queue
    let queue = take_queue_snapshot();

    if queue.is_empty() {
        // No refreshes pending, nothing to persist
        return Ok(());
    }

    info!("TVIEW: Persisting {} refresh requests for prepared transaction '{}'",
          queue.len(), gid);

    // Serialize queue
    let serialized = SerializedQueue::from_queue(queue);
    let queue_jsonb = serialized.to_jsonb();

    // Store in persistent table
    Spi::run_with_args(
        "INSERT INTO pg_tview_pending_refreshes
         (gid, refresh_queue, queue_size, expires_at)
         VALUES ($1, $2, $3, now() + interval '24 hours')",
        Some(vec![
            (PgOid::BuiltIn(PgBuiltInOids::TEXTOID), gid.into_datum()),
            (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), queue_jsonb.into_datum()),
            (PgOid::BuiltIn(PgBuiltInOids::INT4OID), (serialized.keys.len() as i32).into_datum()),
        ]),
    )?;

    // Clear in-memory queue (transaction is prepared, not committed)
    clear_queue();

    Ok(())
}

fn get_prepared_transaction_id() -> TViewResult<String> {
    // Retrieve GID captured by ProcessUtility hook
    PREPARING_GID.lock().unwrap()
        .take() // Take and clear the GID
        .ok_or_else(|| TViewError::InternalError {
            message: "Not in a prepared transaction (GID not captured)".to_string(),
        })
}
```

---

## Phase 8C: COMMIT PREPARED Handling

### Hook Into COMMIT PREPARED

PostgreSQL doesn't provide a direct callback for `COMMIT PREPARED`, so we need to intercept it:

**Option 1: Trigger on pg_prepared_xacts**
```sql
-- Not ideal: pg_prepared_xacts is read-only
```

**Option 2: Custom Function Wrapper**
```sql
-- Wrap COMMIT PREPARED in a function
CREATE FUNCTION pg_tviews_commit_prepared(gid TEXT)
RETURNS VOID AS $$
BEGIN
    -- Process pending refreshes BEFORE committing
    PERFORM pg_tviews_process_prepared(gid);

    -- Then commit the prepared transaction
    EXECUTE format('COMMIT PREPARED %L', gid);
END;
$$ LANGUAGE plpgsql;

-- Usage:
-- Instead of: COMMIT PREPARED 'xact_42';
-- Use:        SELECT pg_tviews_commit_prepared('xact_42');
```

**Option 3: Event Trigger (PostgreSQL 13+)**
```sql
CREATE EVENT TRIGGER tview_commit_prepared_trigger
ON ddl_command_end
WHEN TAG IN ('COMMIT PREPARED')
EXECUTE FUNCTION pg_tviews_commit_prepared_handler();
```

### Implementation (Option 2 - Most Reliable)

**CRITICAL: Refresh order must be: COMMIT PREPARED first, then process queue**

This ensures TVIEWs never reflect uncommitted changes. A small window where base tables are updated but TVIEWs lag is acceptable and safe.

```rust
#[pg_extern]
fn pg_tviews_commit_prepared(gid: &str) -> TViewResult<()> {
    // STEP 1: Load queue metadata BEFORE committing (verify it exists)
    let queue_jsonb: Option<JsonB> = Spi::get_one_with_args(
        "SELECT refresh_queue FROM pg_tview_pending_refreshes WHERE gid = $1",
        vec![(PgOid::BuiltIn(PgBuiltInOids::TEXTOID), gid.into_datum())],
    )?;

    // STEP 2: COMMIT THE PREPARED TRANSACTION FIRST
    // This ensures TVIEWs never show uncommitted data
    let commit_sql = format!("COMMIT PREPARED '{}'", gid);
    Spi::run(&commit_sql)?;

    // STEP 3: Now process the queue (transaction is committed, safe to refresh)
    let queue = match queue_jsonb {
        Some(jsonb) => {
            let serialized = SerializedQueue::from_jsonb(jsonb)?;
            serialized.to_queue()
        }
        None => {
            // No pending refreshes for this GID
            info!("TVIEW: No pending refreshes for prepared transaction '{}'", gid);
            return Ok(());
        }
    };

    if !queue.is_empty() {
        info!("TVIEW: Processing {} deferred refreshes for committed transaction '{}'",
              queue.len(), gid);

        // Process queue in a NEW transaction (prepared transaction already committed)
        Spi::connect(|client| {
            client.run("BEGIN")?;

            match process_refresh_queue(queue) {
                Ok(_) => {
                    client.run("COMMIT")?;
                    Ok(())
                }
                Err(e) => {
                    client.run("ROLLBACK")?;
                    Err(e)
                }
            }
        })?;
    }

    // STEP 4: Clean up persistent entry
    Spi::run_with_args(
        "DELETE FROM pg_tview_pending_refreshes WHERE gid = $1",
        Some(vec![(PgOid::BuiltIn(PgBuiltInOids::TEXTOID), gid.into_datum())]),
    )?;

    Ok(())
}

/// Handle ROLLBACK PREPARED - clean up queue without processing
#[pg_extern]
fn pg_tviews_rollback_prepared(gid: &str) -> TViewResult<()> {
    // STEP 1: Rollback the prepared transaction first
    let rollback_sql = format!("ROLLBACK PREPARED '{}'", gid);
    Spi::run(&rollback_sql)?;

    // STEP 2: Clean up pending queue (no refresh needed - transaction aborted)
    let deleted_count = Spi::get_one_with_args::<i32>(
        "DELETE FROM pg_tview_pending_refreshes WHERE gid = $1 RETURNING 1",
        Some(vec![(PgOid::BuiltIn(PgBuiltInOids::TEXTOID), gid.into_datum())]),
    )?;

    if deleted_count.is_some() {
        info!("TVIEW: Cleaned up pending queue for rolled back transaction '{}'", gid);
    }

    Ok(())
}

/// Process refresh queue (extracted from handle_pre_commit for reuse)
fn process_refresh_queue(queue: HashSet<RefreshKey>) -> TViewResult<()> {
    let mut pending = queue;
    let mut processed = HashSet::new();
    let graph = EntityDepGraph::load_cached()?;

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
            return Err(TViewError::PropagationDepthExceeded {
                max_depth: get_max_propagation_depth(),
                processed: processed.len(),
            });
        }
    }

    Ok(())
}
```

---

## Phase 8D: Automatic Recovery

### Recovery Scenarios

1. **Server Crash**: PostgreSQL restarts, prepared transactions survive
2. **Connection Loss**: Client disconnects, prepared transaction remains
3. **Long-Running Prepared**: Transaction prepared for hours/days

### Recovery Strategy

```sql
-- Function to recover all orphaned prepared transactions
CREATE FUNCTION pg_tviews_recover_prepared_transactions()
RETURNS TABLE (
    gid TEXT,
    queue_size INT,
    status TEXT  -- 'processed', 'skipped', 'error'
) AS $$
SELECT
    gid,
    queue_size,
    CASE
        WHEN pg_tviews_commit_prepared(gid) THEN 'processed'
        ELSE 'error'
    END as status
FROM pg_tview_pending_refreshes
WHERE prepared_at < now() - interval '1 hour'  -- Process old ones first
ORDER BY prepared_at;
$$ LANGUAGE SQL;

-- Schedule via pg_cron (if available)
SELECT cron.schedule('tview-recovery', '*/15 * * * *',
    'SELECT pg_tviews_recover_prepared_transactions()');
```

### Implementation with Advisory Locks

**CRITICAL:** Use advisory locks to prevent concurrent recovery processes (multiple cron jobs).

```rust
#[pg_extern]
fn pg_tviews_recover_prepared_transactions() -> TableIterator<
    'static,
    (
        name!(gid, String),
        name!(queue_size, i32),
        name!(status, String),
    ),
> {
    let results: Vec<(String, i32, String)> = Spi::connect(|client| {
        // Try to acquire advisory lock (non-blocking)
        // Use a fixed hash for the lock key
        const RECOVERY_LOCK_KEY: i64 = 0x7476696577735F72; // "tviews_r" in hex

        let lock_acquired = client.select_one::<bool>(
            &format!("SELECT pg_try_advisory_lock({})", RECOVERY_LOCK_KEY),
            None,
            None,
        )?.unwrap_or(false);

        if !lock_acquired {
            info!("TVIEW: Another recovery process is running, skipping");
            return Ok(Vec::new());
        }

        // Ensure lock is released on exit (even if error occurs)
        let _guard = AdvisoryLockGuard::new(RECOVERY_LOCK_KEY);

        // Perform recovery
        let rows = client.select(
            "SELECT gid, queue_size FROM pg_tview_pending_refreshes
             WHERE prepared_at < now() - interval '1 hour'
             ORDER BY prepared_at",
            None,
            None,
        )?;

        let mut results = Vec::new();

        for row in rows {
            let gid: String = row["gid"].value().unwrap().unwrap();
            let queue_size: i32 = row["queue_size"].value().unwrap().unwrap();

            let status = match pg_tviews_commit_prepared(&gid) {
                Ok(_) => {
                    info!("TVIEW: Recovered prepared transaction '{}' ({} refreshes)", gid, queue_size);
                    "processed".to_string()
                }
                Err(e) => {
                    warning!("TVIEW: Failed to recover prepared transaction '{}': {:?}", gid, e);
                    "error".to_string()
                }
            };

            results.push((gid, queue_size, status));
        }

        Ok::<_, spi::SpiError>(results)
    })
    .unwrap_or_else(|e| {
        error!("TVIEW: Recovery query failed: {:?}", e);
        Vec::new()
    });

    TableIterator::new(results)
}

/// RAII guard for advisory lock (ensures unlock on drop)
struct AdvisoryLockGuard {
    lock_key: i64,
}

impl AdvisoryLockGuard {
    fn new(lock_key: i64) -> Self {
        Self { lock_key }
    }
}

impl Drop for AdvisoryLockGuard {
    fn drop(&mut self) {
        // Release advisory lock
        let _ = Spi::run(&format!("SELECT pg_advisory_unlock({})", self.lock_key));
    }
}
```

---

## Phase 8E: Parallel Refresh

⚠️ **CRITICAL PERFORMANCE WARNING** ⚠️

**This approach blocks the committing transaction** until all background workers complete. For large queues (10K+ rows), this can block the user's `COMMIT` for **10-20 seconds**.

**Alternatives to Consider:**
1. **Async post-commit refresh** (accept eventual consistency)
2. **Configurable threshold** (only parallelize queues > 10K rows)
3. **Time-limited parallelization** (fall back to sequential after 1 second)

**Use Case:** Only beneficial for very large batch operations where user expects long commit time anyway.

### Problem

Current implementation is single-threaded:

```rust
// Sequential processing (slow for large queues)
for key in sorted_keys {
    refresh_and_get_parents(&key)?;  // Blocks until complete
}
```

**Impact**: Large queues (1000+ refreshes) take seconds to process
**Trade-off**: Parallel refresh is faster BUT blocks user transaction during processing

### Solution: Dynamic Background Workers (Recommended)

**RECOMMENDED:** Use **dynamic background workers** spawned on-demand instead of polling workers.

**Advantages:**
- Zero latency (workers start immediately)
- No polling overhead
- Workers auto-terminate when done
- No idle CPU usage

```rust
use pgrx::bgworkers::*;

const PARALLEL_THRESHOLD: usize = 1000;  // Only parallelize for large queues
const MAX_WORKERS: usize = 4;  // Limit concurrent workers

fn handle_pre_commit_parallel() -> TViewResult<()> {
    let queue = take_queue_snapshot();

    if queue.len() < PARALLEL_THRESHOLD {
        // Small queue: process inline (don't spawn workers for small batches)
        return process_refresh_queue(queue);
    }

    info!("TVIEW: Large queue detected ({} items), using parallel refresh", queue.len());

    // Split queue into batches by entity (for parallelization)
    let batches = split_into_batches(queue);
    let worker_count = batches.len().min(MAX_WORKERS);

    info!("TVIEW: Spawning {} workers for parallel refresh", worker_count);

    // Spawn dynamic background workers (one per batch)
    let handles: Vec<BackgroundWorkerHandle> = batches.into_iter()
        .take(worker_count)
        .enumerate()
        .map(|(idx, batch)| {
            BackgroundWorker::spawn_dynamic(
                &format!("pg_tviews_parallel_{}", idx),
                move || {
                    // Each worker processes its batch independently
                    process_refresh_batch(batch)
                }
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Wait for all workers to complete (BLOCKS COMMIT)
    for (idx, handle) in handles.into_iter().enumerate() {
        match handle.wait_with_timeout(Duration::from_secs(30)) {
            Ok(_) => info!("TVIEW: Worker {} completed", idx),
            Err(e) => {
                error!("TVIEW: Worker {} failed or timed out: {:?}", idx, e);
                return Err(TViewError::ParallelRefreshFailed {
                    worker_id: idx,
                    error: format!("{:?}", e),
                });
            }
        }
    }

    Ok(())
}

fn process_refresh_batch(keys: Vec<RefreshKey>) -> TViewResult<()> {
    // Worker function: process batch in its own transaction
    Spi::connect(|client| {
        client.run("BEGIN")?;

        for key in keys {
            refresh_and_get_parents(&key)?;
        }

        client.run("COMMIT")?;
        Ok(())
    })
}
```

### Work Queue Table

```sql
CREATE TABLE pg_tview_work_queue (
    batch_id BIGSERIAL PRIMARY KEY,
    refresh_keys JSONB NOT NULL,  -- Array of RefreshKey
    status TEXT NOT NULL DEFAULT 'pending',  -- pending, processing, completed, failed
    worker_id INT,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    error_message TEXT
);

CREATE INDEX ON pg_tview_work_queue(status) WHERE status = 'pending';
```

### Parallel Processing Logic

```rust
fn handle_pre_commit_parallel() -> TViewResult<()> {
    let queue = take_queue_snapshot();

    if queue.is_empty() {
        return Ok(());
    }

    // Split queue into batches by entity (for parallelization)
    let batches = split_into_batches(queue);

    // Submit batches to work queue
    let batch_ids: Vec<i64> = batches.into_iter().map(|batch| {
        submit_work_batch(batch)
    }).collect::<TViewResult<_>>()?;

    // Wait for all batches to complete (with timeout)
    wait_for_batches(batch_ids, Duration::from_secs(30))?;

    Ok(())
}

fn split_into_batches(queue: HashSet<RefreshKey>) -> Vec<Vec<RefreshKey>> {
    // Group by entity for better parallelization
    let mut groups: HashMap<String, Vec<RefreshKey>> = HashMap::new();
    for key in queue {
        groups.entry(key.entity.clone()).or_default().push(key);
    }

    groups.into_values().collect()
}
```

**Complexity**: High (requires PostgreSQL background worker API)
**Benefit**: 2-4× faster for large queues (>1000 refreshes)

---

## Files to Create/Modify

### Phase 8A: Persistent Queue
- `sql/pg_tview_pending_refreshes.sql` - New: Table schema
- `src/queue/persistence.rs` - New: Serialization logic

### Phase 8B: PREPARE Handling
- `src/queue/xact.rs` - Modify: Add PREPARE event handler
- `src/queue/persistence.rs` - Modify: Add persist_queue()

### Phase 8C: COMMIT PREPARED
- `src/lib.rs` - Modify: Add pg_tviews_commit_prepared() function
- `src/queue/xact.rs` - Modify: Add process_refresh_queue()

### Phase 8D: Recovery
- `src/lib.rs` - Modify: Add pg_tviews_recover_prepared_transactions()
- `sql/pg_tviews_recovery.sql` - New: Recovery procedures

### Phase 8E: Parallel Refresh
- `src/bgworkers.rs` - New: Background worker implementation
- `src/queue/parallel.rs` - New: Parallel processing logic
- `sql/pg_tview_work_queue.sql` - New: Work queue table

---

## Testing Strategy

### Unit Tests
- Queue serialization/deserialization
- Batch splitting logic
- Recovery logic

### Integration Tests
```sql
-- Test 1: PREPARE + COMMIT PREPARED
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
PREPARE TRANSACTION 'test_2pc';
-- Queue persisted to pg_tview_pending_refreshes

-- New connection
SELECT pg_tviews_commit_prepared('test_2pc');
-- Queue restored and processed

-- Test 2: PREPARE + ROLLBACK PREPARED
BEGIN;
UPDATE tb_user SET name = 'Bob' WHERE pk_user = 1;
PREPARE TRANSACTION 'test_rollback';
ROLLBACK PREPARED 'test_rollback';
-- Queue cleaned up

-- Test 3: Recovery
BEGIN;
UPDATE tb_user SET name = 'Charlie' WHERE pk_user = 1;
PREPARE TRANSACTION 'test_recovery';
-- Simulate connection loss (don't commit)
-- Wait 1 hour
SELECT * FROM pg_tviews_recover_prepared_transactions();
-- Should process pending queue

-- Test 4: Parallel refresh (large queue)
BEGIN;
UPDATE tb_user SET name = name || '_updated' WHERE pk_user <= 10000;
COMMIT;
-- Should use parallel processing for 10K refreshes
```

---

## Acceptance Criteria

### Phase 8A: Persistent Queue
- ✅ pg_tview_pending_refreshes table created
- ✅ Queue serialization works correctly
- ✅ JSONB format validated
- ✅ Auto-cleanup removes expired entries

### Phase 8B: PREPARE Handling
- ✅ PREPARE TRANSACTION persists queue
- ✅ Queue cleared from memory after persist
- ✅ Multiple PREPAREs handled correctly
- ✅ Integration tests pass

### Phase 8C: COMMIT PREPARED
- ✅ pg_tviews_commit_prepared() function works
- ✅ Queue restored and processed
- ✅ Persistent entry cleaned up
- ✅ Transaction committed successfully

### Phase 8D: Recovery
- ✅ pg_tviews_recover_prepared_transactions() works
- ✅ Orphaned transactions recovered
- ✅ Error handling for failed recoveries
- ✅ pg_cron integration documented

### Phase 8E: Parallel Refresh
- ✅ Background workers spawn correctly
- ✅ Work queue distributes batches
- ✅ Parallel processing 2-4× faster
- ✅ Error handling for worker failures

---

## Performance Targets

| Scenario | Before Phase 8 | After Phase 8 | Improvement |
|----------|----------------|---------------|-------------|
| 2PC (PREPARE) | Not supported | Supported | **New Feature** |
| Large queue (1000 rows) | 5-10 seconds (sequential) | 2-4 seconds (parallel) | **2-3× faster** |
| Large queue (10000 rows) | 50-100 seconds (sequential) | 15-30 seconds (parallel) | **2-4× faster** |
| Recovery latency | N/A | < 1 second per transaction | **Automated** |

**Note on parallel speedup:**
- **CPU-bound workloads**: 2-4× speedup with 4 workers (typical)
- **I/O-bound workloads**: Speedup depends on disk parallelism (RAID, SSD count)
- **Lock-bound workloads**: May see **NO speedup** or even slowdown due to lock contention
- **Best case**: Independent entities with minimal lock contention = near-linear speedup

---

## Known Limitations After Phase 8

1. **Cross-Database TVIEWs**: Not yet supported (Phase 9)
2. **Federated Queries**: Limited to single database
3. **Background Worker Pool Size**: Fixed at extension load time

---

## Success Metrics

- **2PC Support**: Prepared transactions work correctly
- **Data Consistency**: No stale TVIEWs after 2PC
- **Recovery**: Automatic recovery of orphaned transactions
- **Performance**: Large queues process 5× faster with parallelization
- **Reliability**: No data loss on connection termination

---

## Read Next

- `.phases/phase-8a-persistent-queue.md` - Detailed implementation
- `.phases/phase-8b-prepare-handling.md` - Detailed implementation
- `.phases/phase-8c-commit-prepared.md` - Detailed implementation
- `.phases/phase-8d-recovery.md` - Detailed implementation
- `.phases/phase-8e-parallel-refresh.md` - Detailed implementation

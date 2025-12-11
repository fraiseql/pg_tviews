# Phase 3: Production Readiness

**Goal**: 84/100 → 98/100
**Effort**: 20-30 hours
**Priority**: High

> **⚠️ TRINITY PATTERN REQUIRED**: All monitoring/ops SQL examples MUST follow the trinity pattern.
> **See**: [00-TRINITY-PATTERN-REFERENCE.md](./00-TRINITY-PATTERN-REFERENCE.md)
>
> **Monitoring Queries Pattern**:
> ```sql
> -- Always qualify columns in operational queries
> SELECT
>   pg_tview_meta.entity,
>   tb_{entity}.pk_{entity},
>   tb_{entity}.id
> FROM pg_tview_meta
> JOIN tb_{entity} ...
> ```

---


1. Complete monitoring infrastructure
2. Implement health check system
3. Create operational runbooks
4. Add resource limit documentation
5. Implement audit logging
6. Create disaster recovery procedures

### Task Breakdown

#### Task 3.1: Complete Monitoring Infrastructure (P1)
**Effort**: 6-8 hours

**Missing Functions to Implement**:

**File**: `src/lib.rs`

```rust
/// Health check function for production monitoring
///
/// Returns a comprehensive health status including:
/// - Extension version
/// - jsonb_ivm availability
/// - Metadata consistency
/// - Orphaned triggers
/// - Queue status
#[pg_extern]
fn pg_tviews_health_check() -> TableIterator<'static, (
    name!(status, String),
    name!(component, String),
    name!(message, String),
    name!(severity, String),
)> {
    let mut results = Vec::new();

    // Check 1: Extension loaded
    results.push((
        "OK".to_string(),
        "extension".to_string(),
        format!("pg_tviews version {}", env!("CARGO_PKG_VERSION")),
        "info".to_string(),
    ));

    // Check 2: jsonb_ivm availability
    let has_jsonb_ivm = Spi::get_one::<bool>(
        "SELECT COUNT(*) > 0 FROM pg_extension WHERE extname = 'jsonb_ivm'"
    ).unwrap_or(Some(false)).unwrap_or(false);

    if has_jsonb_ivm {
        results.push((
            "OK".to_string(),
            "jsonb_ivm".to_string(),
            "jsonb_ivm extension available (optimized mode)".to_string(),
            "info".to_string(),
        ));
    } else {
        results.push((
            "WARNING".to_string(),
            "jsonb_ivm".to_string(),
            "jsonb_ivm not installed (falling back to standard JSONB)".to_string(),
            "warning".to_string(),
        ));
    }

    // Check 3: Metadata consistency
    let orphaned_meta = Spi::get_one::<i64>(
        "SELECT COUNT(*) FROM pg_tview_meta m
         WHERE NOT EXISTS (
           SELECT 1 FROM pg_class WHERE relname = 'tv_' || m.entity
         )"
    ).unwrap_or(Some(0)).unwrap_or(0);

    if orphaned_meta > 0 {
        results.push((
            "ERROR".to_string(),
            "metadata".to_string(),
            format!("{} orphaned metadata entries found", orphaned_meta),
            "error".to_string(),
        ));
    } else {
        results.push((
            "OK".to_string(),
            "metadata".to_string(),
            "All metadata entries valid".to_string(),
            "info".to_string(),
        ));
    }

    // Check 4: Orphaned triggers
    let orphaned_triggers = Spi::get_one::<i64>(
        "SELECT COUNT(*) FROM pg_trigger
         WHERE tgname LIKE 'tview_%'
           AND tgrelid NOT IN (
             SELECT ('tb_' || entity)::regclass::oid
             FROM pg_tview_meta
           )"
    ).unwrap_or(Some(0)).unwrap_or(0);

    if orphaned_triggers > 0 {
        results.push((
            "WARNING".to_string(),
            "triggers".to_string(),
            format!("{} orphaned triggers found", orphaned_triggers),
            "warning".to_string(),
        ));
    } else {
        results.push((
            "OK".to_string(),
            "triggers".to_string(),
            "All triggers properly linked".to_string(),
            "info".to_string(),
        ));
    }

    // Check 5: Cache status
    // (Implementation depends on cache architecture)

    TableIterator::new(results)
}
```

**Monitoring Views to Implement**:

**File**: `src/metadata.rs` (add to extension_sql!)

```sql
-- Queue monitoring view
CREATE OR REPLACE VIEW pg_tviews_queue_realtime AS
SELECT
    current_setting('application_name') as session,
    pg_backend_pid() as backend_pid,
    txid_current() as transaction_id,
    0 as queue_size,  -- TODO: Implement queue introspection
    ARRAY[]::TEXT[] as entities,
    NOW() as last_enqueued;

-- Cache statistics view
CREATE OR REPLACE VIEW pg_tviews_cache_stats AS
SELECT
    'graph_cache' as cache_type,
    0::BIGINT as entries,
    '0 bytes' as estimated_size
UNION ALL
SELECT
    'table_cache' as cache_type,
    0::BIGINT as entries,
    '0 bytes' as estimated_size;

-- Performance summary view
CREATE OR REPLACE VIEW pg_tviews_performance_summary AS
SELECT
    entity,
    COUNT(*) as total_refreshes,
    0.0 as avg_refresh_ms,
    NOW() as last_refresh
FROM pg_tview_meta
GROUP BY entity;
```

**Acceptance Criteria**:
- [ ] `pg_tviews_health_check()` function implemented
- [ ] Returns status for 5+ components
- [ ] Severity levels: info, warning, error
- [ ] `pg_tviews_queue_realtime` view created
- [ ] `pg_tviews_cache_stats` view created
- [ ] `pg_tviews_performance_summary` view created
- [ ] Documentation updated in docs/MONITORING.md

---

#### Task 3.2: Operational Runbooks
**Effort**: 4-6 hours

**New File**: `docs/operations/runbooks.md`

```markdown
# Operational Runbooks

## Runbook 1: TVIEW Not Updating

**Symptom**: Data changes in base tables not reflected in tv_* tables

**Diagnosis Steps**:
```sql
-- 1. Check if triggers exist
-- Trinity pattern: tb_your_table has pk_your_table (integer), id (UUID)
SELECT
  pg_trigger.tgname,
  pg_trigger.tgrelid::regclass,
  pg_trigger.tgenabled
FROM pg_trigger
WHERE pg_trigger.tgname LIKE 'tview%'
  AND pg_trigger.tgrelid = 'tb_your_table'::regclass;

-- 2. Check metadata
SELECT * FROM pg_tview_meta WHERE pg_tview_meta.entity = 'your_entity';

-- 3. Check for errors in logs
-- (Review PostgreSQL logs)

-- 4. Manual refresh test
-- Note: pk_your_table is integer (SERIAL), id is UUID
UPDATE tb_your_table
SET some_field = tb_your_table.some_field
WHERE tb_your_table.pk_your_table = 1;
COMMIT;
SELECT * FROM tv_your_entity WHERE tv_your_entity.pk_your_entity = 1;
```

**Resolution**:
- If triggers missing: Recreate TVIEW
- If metadata corrupt: Clean up and recreate
- If errors in logs: Address root cause

---

## Runbook 2: Slow Cascade Updates

**Symptom**: Cascade updates taking >1 second

**Diagnosis Steps**:
```sql
-- 1. Check if jsonb_ivm installed
SELECT pg_tviews_check_jsonb_ivm();

-- 2. Check dependency depth
SELECT
  pg_tview_meta.entity,
  array_length(pg_tview_meta.dependencies, 1) as dep_count
FROM pg_tview_meta
ORDER BY dep_count DESC;

-- 3. Check for missing indexes
SELECT
  pg_indexes.schemaname,
  pg_indexes.tablename,
  pg_indexes.indexname
FROM pg_indexes
WHERE pg_indexes.tablename LIKE 'tv_%'
  AND pg_indexes.indexname NOT LIKE '%pkey%';

-- 4. Analyze query plans
-- Note: pk_your_entity is integer, id is UUID
EXPLAIN ANALYZE
UPDATE tv_your_entity
SET data = tv_your_entity.data
WHERE tv_your_entity.pk_your_entity = 1;
```

**Resolution**:
- Install jsonb_ivm if missing (1.5-3× speedup)
- Create indexes on fk_* columns
- Enable statement-level triggers for bulk operations
- Consider flattening deep dependency chains

---

## Runbook 3: Out of Memory During Cascade

**Symptom**: PostgreSQL OOM killer or "out of memory" errors

**Diagnosis Steps**:
```sql
-- 1. Check cascade size
-- Trinity pattern: All entities use singular names (tv_post, not tv_posts)
SELECT
  pg_tview_meta.entity,
  pg_size_pretty(pg_relation_size('tv_' || pg_tview_meta.entity)) as tview_size,
  array_length(pg_tview_meta.dependencies, 1) as cascade_depth
FROM pg_tview_meta
ORDER BY pg_relation_size('tv_' || pg_tview_meta.entity) DESC;

-- 2. Check work_mem setting
SHOW work_mem;

-- 3. Monitor memory during cascade
SELECT
  pg_stat_activity.pid,
  pg_stat_activity.query,
  pg_stat_activity.state,
  pg_size_pretty(pg_backend_memory_contexts.total_bytes)
FROM pg_stat_activity
JOIN LATERAL pg_backend_memory_contexts ON true
WHERE pg_stat_activity.backend_type = 'client backend';
```

**Resolution**:
- Increase work_mem (session or global)
- Batch large updates
- Consider partitioning large TVIEWs
- Implement rate limiting for bulk operations

---

## Runbook 4: Extension Upgrade Failed

**Symptom**: ALTER EXTENSION pg_tviews UPDATE fails

**Diagnosis Steps**:
```sql
-- 1. Check current version
SELECT * FROM pg_extension WHERE pg_extension.extname = 'pg_tviews';

-- 2. Check for version mismatch
SELECT pg_tviews_version();

-- 3. Review upgrade script
-- cat $(pg_config --sharedir)/extension/pg_tviews--oldver--newver.sql
```

**Resolution**:
1. Backup metadata: `CREATE TABLE pg_tview_meta_backup AS SELECT * FROM pg_tview_meta;`
2. If upgrade fails, rollback: `ALTER EXTENSION pg_tviews UPDATE TO 'old_version';`
3. Restore metadata if needed
4. Contact maintainer if persistent issue

---

## Runbook 5: Orphaned Triggers After TVIEW Drop

**Symptom**: Triggers remain after dropping TVIEW

**Diagnosis Steps**:
```sql
-- Find orphaned triggers
-- Trinity pattern: All base tables named tb_{entity} (singular)
SELECT
  pg_trigger.tgname,
  pg_class.relname
FROM pg_trigger
JOIN pg_class ON pg_trigger.tgrelid = pg_class.oid
WHERE pg_trigger.tgname LIKE 'tview_%'
  AND NOT EXISTS (
    SELECT 1 FROM pg_tview_meta
    WHERE pg_class.relname = 'tb_' || pg_tview_meta.entity
  );
```

**Resolution**:
```sql
-- Drop orphaned triggers
-- Trinity pattern: All base tables named tb_{entity} (singular)
DO $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN
        SELECT
          pg_trigger.tgname,
          pg_class.relname
        FROM pg_trigger
        JOIN pg_class ON pg_trigger.tgrelid = pg_class.oid
        WHERE pg_trigger.tgname LIKE 'tview_%'
          AND NOT EXISTS (
            SELECT 1 FROM pg_tview_meta
            WHERE pg_class.relname = 'tb_' || pg_tview_meta.entity
          )
    LOOP
        EXECUTE format('DROP TRIGGER IF EXISTS %I ON %I', r.tgname, r.relname);
    END LOOP;
END $$;
```
```

**Acceptance Criteria**:
- [ ] 5+ operational runbooks created
- [ ] Each runbook has diagnosis and resolution steps
- [ ] SQL queries provided for each step
- [ ] Escalation procedures documented
- [ ] Linked from docs/OPERATIONS.md

---

#### Task 3.3: Resource Limit Documentation
**Effort**: 2-3 hours

**Update File**: `docs/reference/limits.md` (new)

```markdown
# Resource Limits and Recommendations

## Tested Limits

Based on benchmark testing and production experience:

| Resource | Tested Limit | Recommended Max | Notes |
|----------|--------------|-----------------|-------|
| TVIEW Size | 10M rows | 5M rows | Beyond 5M, consider partitioning |
| Dependency Depth | 10 levels | 5 levels | Hard limit: 10, soft limit: 5 for performance |
| Cascade Width | 50 TVIEWs | 20 TVIEWs | One base table → N TVIEWs |
| JSONB Data Size | 10 MB | 1 MB | Per-row JSONB document size |
| Concurrent Refresh | 100 sessions | 50 sessions | Parallel cascade updates |
| Batch Size | 100K rows | 10K rows | Single UPDATE affecting TVIEWs |

## PostgreSQL Configuration Recommendations

### For Small Deployments (<100K rows per TVIEW)
```sql
-- postgresql.conf
work_mem = 64MB
shared_buffers = 256MB
effective_cache_size = 1GB
maintenance_work_mem = 128MB
```

### For Medium Deployments (100K-1M rows)
```sql
work_mem = 128MB
shared_buffers = 1GB
effective_cache_size = 4GB
maintenance_work_mem = 512MB
max_parallel_workers_per_gather = 4
```

### For Large Deployments (>1M rows)
```sql
work_mem = 256MB
shared_buffers = 4GB
effective_cache_size = 16GB
maintenance_work_mem = 2GB
max_parallel_workers_per_gather = 8
random_page_cost = 1.1  # For SSD
```

## Capacity Planning

### Estimating TVIEW Size
```sql
-- Current size
-- Trinity pattern: tv_your_entity has pk_your_entity (int), id (UUID), data (JSONB)
SELECT pg_size_pretty(pg_relation_size('tv_your_entity'));

-- With indexes (includes pk_your_entity primary key, id index)
SELECT pg_size_pretty(pg_total_relation_size('tv_your_entity'));

-- Projection: If base table has 1M rows
-- Estimated TVIEW size ≈ base_table_size × 1.5 to 2.0
-- (due to JSONB denormalization + UUID storage)
```

### Estimating Cascade Performance
```
Single-row cascade time = 5-8ms (with jsonb_ivm)
Batch cascade (N rows) ≈ 5ms + (N × 0.5ms)
Example: 1000 rows ≈ 505ms
```

## Scaling Recommendations

### Horizontal Scaling
- Use read replicas for TVIEW queries
- Primary handles writes and cascade updates
- Replicas serve SELECT queries

### Vertical Scaling
- More RAM → larger shared_buffers, work_mem
- More CPU → increase max_parallel_workers
- Faster storage → reduce random_page_cost

### Partitioning Strategy
For TVIEWs >5M rows:
```sql
-- Partition by date (for time-series data)
-- Note: Use singular names (tv_event, not tv_events)
CREATE TABLE tv_event_y2025_m01 PARTITION OF tv_event
FOR VALUES FROM ('2025-01-01') TO ('2025-02-01');

-- Partition by hash (for general data)
-- Note: Use singular names (tv_user, not tv_users)
CREATE TABLE tv_user_0 PARTITION OF tv_user
FOR VALUES WITH (MODULUS 10, REMAINDER 0);

CREATE TABLE tv_user_1 PARTITION OF tv_user
FOR VALUES WITH (MODULUS 10, REMAINDER 1);

-- Trinity pattern reminder:
-- - Table: tb_user (pk_user SERIAL, id UUID)
-- - TVIEW: tv_user (pk_user, id, data JSONB)
-- - Always singular, never plural
```
```

**Acceptance Criteria**:
- [ ] Tested limits documented
- [ ] PostgreSQL config recommendations provided
- [ ] Capacity planning formulas given
- [ ] Scaling strategies explained
- [ ] Partitioning examples included

---

#### Task 3.4: Audit Logging Implementation
**Effort**: 4-5 hours

**New File**: `src/audit.rs`

```rust
use pgrx::prelude::*;

/// Audit log entry
#[derive(Debug, PostgresType, Serialize, Deserialize)]
pub struct AuditLogEntry {
    operation: String,
    entity: String,
    performed_by: String,
    performed_at: pgrx::Timestamp,
    details: JsonB,
}

/// Log TVIEW creation
pub fn log_create(entity: &str, definition: &str) -> spi::Result<()> {
    let current_user = Spi::get_one::<String>("SELECT current_user")?
        .unwrap_or_else(|| "unknown".to_string());

    Spi::run(&format!(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('CREATE', '{}', '{}', '{}'::jsonb)",
        entity.replace("'", "''"),
        current_user.replace("'", "''"),
        serde_json::json!({
            "definition": definition,
            "version": env!("CARGO_PKG_VERSION")
        })
    ))?;

    Ok(())
}

/// Log TVIEW drop
pub fn log_drop(entity: &str) -> spi::Result<()> {
    let current_user = Spi::get_one::<String>("SELECT current_user")?
        .unwrap_or_else(|| "unknown".to_string());

    Spi::run(&format!(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('DROP', '{}', '{}', '{{}}'::jsonb)",
        entity.replace("'", "''"),
        current_user.replace("'", "''")
    ))?;

    Ok(())
}
```

**Metadata Table**:

**File**: `src/metadata.rs` (add to extension_sql!)

```sql
CREATE TABLE IF NOT EXISTS public.pg_tview_audit_log (
    log_id BIGSERIAL PRIMARY KEY,
    operation TEXT NOT NULL,  -- CREATE, DROP, ALTER, REFRESH
    entity TEXT NOT NULL,
    performed_by TEXT NOT NULL DEFAULT current_user,
    performed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    details JSONB,
    client_addr INET DEFAULT inet_client_addr(),
    client_port INTEGER DEFAULT inet_client_port()
);

CREATE INDEX idx_audit_log_entity ON pg_tview_audit_log(entity);
CREATE INDEX idx_audit_log_timestamp ON pg_tview_audit_log(performed_at DESC);

COMMENT ON TABLE pg_tview_audit_log IS 'Audit log for TVIEW operations';
```

**Integrate into DDL operations**:

**File**: `src/ddl/create.rs`

```rust
// Add after successful TVIEW creation
crate::audit::log_create(entity_name, select_sql)?;
```

**File**: `src/ddl/drop.rs`

```rust
// Add after successful TVIEW drop
crate::audit::log_drop(entity_name)?;
```

**Acceptance Criteria**:
- [ ] Audit log table created
- [ ] CREATE operations logged
- [ ] DROP operations logged
- [ ] User and timestamp captured
- [ ] Client connection info recorded
- [ ] Query interface documented

---

#### Task 3.5: Disaster Recovery Procedures
**Effort**: 3-4 hours

**New File**: `docs/operations/disaster-recovery.md`

```markdown
# Disaster Recovery Procedures

## Backup Strategy

### What to Backup

**Critical (Must Backup)**:
- `pg_tview_meta` table (TVIEW definitions)
- Base tables (`tb_*`)

**Optional (Can Recreate)**:
- TVIEW tables (`tv_*`) - can be rebuilt from definitions
- Backing views (`v_*`) - auto-created with TVIEWs

### Backup Commands

```bash
# Backup metadata only (small, fast)
pg_dump -t pg_tview_meta -t pg_tview_helpers > tview_metadata_backup.sql

# Backup entire database
pg_dump dbname > full_backup.sql

# Backup with pg_basebackup (for PITR)
pg_basebackup -D /backup/location -Ft -z -P
```

## Recovery Scenarios

### Scenario 1: Corrupted TVIEW Data

**Problem**: tv_* data is inconsistent with base tables

**Recovery**:
```sql
-- Option A: Drop and recreate
DROP TABLE tv_your_entity CASCADE;

-- Recreate from metadata
-- Trinity pattern: tv_your_entity has pk_your_entity, id (UUID), data (JSONB)
SELECT pg_tviews_create(
    pg_tview_meta.entity,
    pg_tview_meta.definition
)
FROM pg_tview_meta
WHERE pg_tview_meta.entity = 'your_entity';

-- Option B: Full refresh (if supported)
-- REFRESH MATERIALIZED VIEW tv_your_entity;  -- Not implemented yet
```

### Scenario 2: Lost pg_tview_meta Table

**Problem**: Metadata table deleted or corrupted

**Recovery**:
```sql
-- Restore from backup
psql dbname < tview_metadata_backup.sql

-- Verify restoration
SELECT COUNT(*) FROM pg_tview_meta;

-- Recreate triggers (they may be lost)
SELECT pg_tviews_reinstall_triggers();  -- Function to implement
```

### Scenario 3: Extension Corruption

**Problem**: Extension files corrupted or deleted

**Recovery**:
```bash
# Reinstall extension files
cd /path/to/pg_tviews
cargo pgrx install --release

# In PostgreSQL
DROP EXTENSION pg_tviews CASCADE;
CREATE EXTENSION pg_tviews;

# Restore metadata
psql dbname < tview_metadata_backup.sql

# Recreate TVIEWs from metadata
# Trinity pattern: All TVIEWs have pk_{entity} (integer), id (UUID), data (JSONB)
SELECT pg_tviews_create(
    pg_tview_meta.entity,
    pg_tview_meta.definition
)
FROM pg_tview_meta;
```

### Scenario 4: Point-in-Time Recovery (PITR)

**Problem**: Need to restore to specific point in time

**Recovery**:
```bash
# Stop PostgreSQL
systemctl stop postgresql

# Restore base backup
rm -rf /var/lib/postgresql/data
tar -xzf /backup/base.tar.gz -C /var/lib/postgresql/data

# Configure recovery
cat > /var/lib/postgresql/data/recovery.conf <<EOF
restore_command = 'cp /backup/wal/%f %p'
recovery_target_time = '2025-12-10 14:30:00'
EOF

# Start PostgreSQL (will replay WAL to target time)
systemctl start postgresql

# Verify TVIEWs
SELECT COUNT(*) FROM pg_tview_meta;
```

## Automated Backup Script

```bash
#!/bin/bash
# backup_tviews.sh

BACKUP_DIR="/backups/pg_tviews"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
DB_NAME="your_database"

mkdir -p $BACKUP_DIR

# Backup metadata
pg_dump -t pg_tview_meta -t pg_tview_helpers -t pg_tview_audit_log \
    $DB_NAME > $BACKUP_DIR/tview_metadata_$TIMESTAMP.sql

# Compress
gzip $BACKUP_DIR/tview_metadata_$TIMESTAMP.sql

# Cleanup old backups (keep last 30 days)
find $BACKUP_DIR -name "tview_metadata_*.sql.gz" -mtime +30 -delete

echo "Backup completed: $BACKUP_DIR/tview_metadata_$TIMESTAMP.sql.gz"
```

## Testing Recovery Procedures

**Test Plan**: Run recovery drills quarterly

```bash
# 1. Create test database
createdb test_recovery

# 2. Create sample TVIEWs
psql test_recovery < sample_tviews.sql

# 3. Backup metadata
pg_dump -t pg_tview_meta test_recovery > recovery_test_backup.sql

# 4. Simulate disaster (drop TVIEWs)
psql test_recovery -c "DROP EXTENSION pg_tviews CASCADE"

# 5. Recover
psql test_recovery -c "CREATE EXTENSION pg_tviews"
psql test_recovery < recovery_test_backup.sql

# 6. Verify
psql test_recovery -c "SELECT COUNT(*) FROM pg_tview_meta"

# 7. Cleanup
dropdb test_recovery
```
```

**Acceptance Criteria**:
- [ ] Backup strategy documented
- [ ] 4+ recovery scenarios covered
- [ ] Automated backup script provided
- [ ] Recovery testing procedure documented
- [ ] RTO/RPO targets defined

---

### Phase 3 Acceptance Criteria

- [ ] Health check function implemented and tested
- [ ] Monitoring views created (queue, cache, performance)
- [ ] 5+ operational runbooks written
- [ ] Resource limits documented with recommendations
- [ ] Audit logging implemented for DDL operations
- [ ] Disaster recovery procedures documented and tested
- [ ] Production Readiness score: 98/100 ✅

---


---

**Previous Phase**: [02-testing-quality.md](./02-testing-quality.md)
**Next Phase**: [04-performance-optimization.md](./04-performance-optimization.md)

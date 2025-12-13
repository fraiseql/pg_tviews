# Phase 5.1: Operations Runbooks

**Objective**: Create comprehensive operational runbooks for all common pg_tviews scenarios

**Priority**: MEDIUM
**Estimated Time**: 1-2 days
**Blockers**: Phase 2, 3 complete

---

## Context

**Current State**: Limited operational documentation for production deployments

**Why This Matters**:
- On-call engineers need clear, executable procedures
- Runbooks reduce MTTR (Mean Time To Resolution) during incidents
- Operational knowledge shouldn't depend on single team member
- Consistent procedures prevent mistakes under pressure

**Deliverable**: Complete ops runbook library with tested procedures for daily operations

---

## Runbooks to Create

### Category 1: Health and Monitoring

1. **TVIEW Health Check**
   - Verify TVIEWs are in sync
   - Check queue status
   - Monitor refresh performance

2. **Queue Management**
   - View queue status
   - Clear orphaned entries
   - Monitor 2PC transactions

3. **Performance Monitoring**
   - Track refresh times
   - Monitor memory usage
   - Check disk I/O

### Category 2: Refresh Operations

4. **Manual Refresh**
   - Refresh single TVIEW
   - Refresh all TVIEWs
   - Force refresh with validation

5. **Batch Refresh Operations**
   - Refresh by dependency level
   - Refresh with rate limiting
   - Partial refresh (large TVIEWs)

6. **Refresh Troubleshooting**
   - Debug slow refresh
   - Handle refresh failures
   - Retry failed refreshes

### Category 3: Maintenance

7. **Regular Maintenance**
   - VACUUM metadata tables
   - Analyze performance data
   - Rotate logs

8. **Connection Management**
   - Monitor active connections
   - Terminate stale connections
   - Handle connection limits

9. **Table Analysis**
   - Gather statistics
   - Identify bloat
   - Reindex if needed

### Category 4: Incident Response

10. **Emergency Procedures**
    - Disable TVIEWs during crisis
    - Emergency refresh
    - Fallback to read-only mode

---

## Implementation Steps

### Step 1: Create Ops Runbook Directory Structure

**Create**: `docs/operations/runbooks/`

```
docs/operations/runbooks/
├── README.md
├── 01-health-monitoring/
│   ├── tview-health-check.md
│   ├── queue-management.md
│   └── performance-monitoring.md
├── 02-refresh-operations/
│   ├── manual-refresh.md
│   ├── batch-refresh.md
│   └── refresh-troubleshooting.md
├── 03-maintenance/
│   ├── regular-maintenance.md
│   ├── connection-management.md
│   └── table-analysis.md
├── 04-incident-response/
│   ├── emergency-procedures.md
│   ├── incident-checklist.md
│   └── post-incident-review.md
└── scripts/
    ├── health-check.sql
    ├── refresh-status.sql
    ├── queue-cleanup.sql
    └── emergency-disable.sql
```

### Step 2: Health Monitoring Runbook

**Create**: `docs/operations/runbooks/01-health-monitoring/tview-health-check.md`

```markdown
# TVIEW Health Check Runbook

## Purpose
Regular health check to ensure all TVIEWs are synchronized and operational.

## Frequency
- Every 4 hours during business hours
- After major data changes
- After PostgreSQL maintenance
- When users report sync issues

## Prerequisites
- PostgreSQL CLI access to production database
- psql installed locally
- Database credentials with SELECT on system tables

## Procedure

### Quick Check (2 minutes)
Run this to get a quick status:

```sql
-- Check 1: All TVIEWs are defined
SELECT COUNT(*) as tview_count FROM pg_tviews_metadata;

-- Check 2: No TVIEWs in error state
SELECT entity_name FROM pg_tviews_metadata WHERE last_error IS NOT NULL;

-- Check 3: Queue is empty
SELECT COUNT(*) as queue_size FROM pg_tviews_get_queue();

-- Check 4: Recent refresh times
SELECT entity_name, last_refresh_time,
  EXTRACT(EPOCH FROM (now() - last_refresh_time)) as seconds_ago
FROM pg_tviews_metadata
ORDER BY last_refresh_time DESC
LIMIT 10;
```

### Full Health Check (5-10 minutes)

```sql
-- Full diagnostics
WITH tview_stats AS (
  SELECT
    entity_name,
    backing_table_name,
    last_refresh_time,
    last_error,
    (SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES
     WHERE TABLE_NAME = backing_table_name) as backing_exists,
    (SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES
     WHERE TABLE_NAME = entity_name) as tview_exists
  FROM pg_tviews_metadata
)
SELECT
  entity_name,
  CASE
    WHEN backing_exists = 0 THEN '❌ BACKING TABLE MISSING'
    WHEN tview_exists = 0 THEN '❌ TVIEW MISSING'
    WHEN last_error IS NOT NULL THEN '⚠️  ERROR: ' || last_error
    WHEN (now() - last_refresh_time) > interval '1 hour' THEN '⚠️  STALE'
    ELSE '✅ OK'
  END as status,
  EXTRACT(EPOCH FROM (now() - last_refresh_time)) as seconds_since_refresh
FROM tview_stats
ORDER BY entity_name;

-- Queue depth and 2PC status
SELECT COUNT(*) as queue_size FROM pg_tviews_get_queue();
SELECT COUNT(*) as prepared_xacts FROM pg_prepared_xacts;

-- Active refresh transactions
SELECT pid, usename, state, query, query_start
FROM pg_stat_activity
WHERE query ILIKE '%tview%' AND state != 'idle'
ORDER BY query_start DESC;
```

### Interpretation

**✅ OK**: TVIEW is synchronized
**⚠️  STALE**: Last refresh > 1 hour ago (may indicate slow backing table)
**⚠️  ERROR**: Last_error is set (refresh failed)
**❌ CRITICAL**: Table missing or queue stuck

### Actions by Status

#### Stale TVIEW (> 1 hour)
```bash
# Check if backing table has changed
psql -c "SELECT COUNT(*) FROM backing_table_name;" > /tmp/count1.txt
sleep 10
psql -c "SELECT COUNT(*) FROM backing_table_name;" > /tmp/count2.txt
diff /tmp/count1.txt /tmp/count2.txt

# If changed: manually trigger refresh
psql -c "SELECT pg_tviews_refresh('entity_name');"

# If unchanged: backing table may be frozen, check:
psql -c "SELECT xmin, xmax FROM backing_table_name LIMIT 1;"
```

#### TVIEW in Error State
```bash
# View error details
psql -c "SELECT last_error FROM pg_tviews_metadata WHERE entity_name = 'entity_name';"

# For connection errors: wait and retry
psql -c "SELECT pg_tviews_refresh('entity_name');"

# For data errors: investigate backing table
psql -c "SELECT * FROM backing_table_name WHERE ... LIMIT 1;"
```

#### Queue Stuck (> 1000 entries)
```bash
# View queue
SELECT * FROM pg_tviews_get_queue() LIMIT 50;

# Check if 2PC transactions are preparing
SELECT * FROM pg_prepared_xacts;

# If stuck for > 5 minutes, may need manual cleanup (see Queue Management runbook)
```

## Success Criteria
- ✅ All TVIEWs report status ✅ OK
- ✅ Queue size is 0 (or < 10 temporarily)
- ✅ No ERROR states
- ✅ All TVIEW tables exist
- ✅ Last refresh time is recent (< 1 hour)

## If Issues Found
→ Go to relevant troubleshooting runbook
- Stale TVIEW → Refresh Troubleshooting
- Error state → Incident Response
- Queue stuck → Queue Management

## Notes
- This runbook is non-invasive (read-only queries only)
- Safe to run multiple times
- Can be automated with cron job
```

### Step 3: Queue Management Runbook

**Create**: `docs/operations/runbooks/01-health-monitoring/queue-management.md`

```markdown
# Queue Management Runbook

## Purpose
Monitor and manage the pg_tviews refresh queue and 2PC transactions.

## When to Use
- Monitoring queue depth in production
- Clearing orphaned queue entries
- Investigating stuck transactions
- After database crashes

## Prerequisites
- Database superuser access (for cleanup operations)
- psql CLI
- Understanding of 2PC (two-phase commit)

## Monitoring Queue

### View Queue Status
```sql
-- Simple queue depth
SELECT COUNT(*) as queue_size FROM pg_tviews_get_queue();

-- Queue details
SELECT * FROM pg_tviews_get_queue();

-- Queue by TVIEW
SELECT entity_name, COUNT(*) as queued_refreshes
FROM pg_tviews_get_queue()
GROUP BY entity_name
ORDER BY COUNT(*) DESC;
```

### Healthy Queue Behavior
- Size: 0-5 entries (most of the time)
- Size: 0-50 during high load
- Entries processed within seconds
- No entries older than 5 minutes

### Queue Warning Signs
- **Size > 100**: Refresh operations slow, backlog building
- **Size > 1000**: Serious backlog, refresh likely stuck
- **Age > 10 minutes**: Individual refresh stuck
- **Age > 1 hour**: Critical - queue definitely stuck

## Managing 2PC Transactions

### View Prepared Transactions
```sql
-- List all prepared transactions
SELECT gid, prepared, owner FROM pg_prepared_xacts;

-- Prepared transactions older than 1 hour
SELECT gid, prepared, owner,
  EXTRACT(EPOCH FROM (now() - prepared)) as age_seconds
FROM pg_prepared_xacts
WHERE (now() - prepared) > interval '1 hour'
ORDER BY prepared ASC;
```

### Understanding 2PC State

Normal flow:
```
INSERT → PREPARED → COMMITTED → Removed
        (2PC Phase 1)  (2PC Phase 2)
```

If in PREPARED state > 1 hour:
- Either client never committed/rolled back
- Or PostgreSQL crashed mid-2PC

### Recovering Stuck 2PC Transactions

**⚠️  WARNING: Only do this if you understand the implications**

```sql
-- Option 1: Commit orphaned transaction (if data is known good)
COMMIT PREPARED 'gid_value';

-- Option 2: Rollback orphaned transaction (if data is suspect)
ROLLBACK PREPARED 'gid_value';

-- Option 3: Keep watching if recent (< 5 minutes)
-- Often will complete naturally
```

## Cleaning Orphaned Queue Entries

### Identify Orphaned Entries
```sql
-- Queue entries for non-existent TVIEWs
SELECT q.entity_name
FROM pg_tviews_get_queue() q
LEFT JOIN pg_tviews_metadata m ON q.entity_name = m.entity_name
WHERE m.entity_name IS NULL;

-- Very old queue entries (> 30 minutes)
SELECT * FROM pg_tviews_get_queue()
WHERE entry_age > interval '30 minutes';
```

### Safe Cleanup Procedure

**Step 1: Verify entries are truly orphaned**
```bash
# Confirm TVIEWs don't exist
psql -c "SELECT COUNT(*) FROM pg_tviews_metadata WHERE entity_name = 'entity_name';"
# Should return 0
```

**Step 2: Drain queue safely**
```sql
-- Pause refresh triggers (stops new queue entries)
-- See Emergency Procedures for procedure

-- Wait 30 seconds for in-progress refreshes
SELECT pg_sleep(30);

-- Check if queue is now empty
SELECT COUNT(*) FROM pg_tviews_get_queue();

-- If still has entries for deleted TVIEWs, truncate queue table
-- (requires knowing internal queue table name)
```

**Step 3: Resume operations**
```sql
-- Re-enable triggers
-- See Emergency Procedures
```

## Monitoring Script

**Create**: `docs/operations/runbooks/scripts/queue-monitor.sh`

```bash
#!/bin/bash
set -euo pipefail

# Run every minute, alert if queue > 100 or age > 10 minutes

while true; do
  psql -tAc "
    SELECT queue_depth, max_age_minutes,
      CASE
        WHEN queue_depth > 1000 THEN 'CRITICAL'
        WHEN queue_depth > 100 THEN 'WARNING'
        WHEN max_age_minutes > 10 THEN 'WARNING'
        ELSE 'OK'
      END as status
    FROM (
      SELECT
        COUNT(*) as queue_depth,
        COALESCE(EXTRACT(EPOCH FROM (now() - MIN(entry_time)))/60, 0) as max_age_minutes
      FROM pg_tviews_get_queue()
    ) stats;
  " | while IFS='|' read queue_depth max_age status; do
    echo "$(date): Queue=$queue_depth, Max Age=${max_age}min, Status=$status"

    if [ "$status" != "OK" ]; then
      # Send alert
      echo "ALERT: Queue status is $status" | mail -s "pg_tviews Queue Alert" ops@company.com
    fi
  done

  sleep 60
done
```

## Automation

To run queue health check every hour:

```bash
# Create cron job
(crontab -l 2>/dev/null; echo "0 * * * * /opt/pg_tviews/queue-monitor.sh >> /var/log/pg_tviews/queue.log 2>&1") | crontab -
```

## Success Criteria
- ✅ Queue size < 50
- ✅ Queue age < 5 minutes
- ✅ No orphaned 2PC transactions
- ✅ All entries processed regularly

## References
- [PostgreSQL 2PC Documentation](https://www.postgresql.org/docs/current/sql-commit-prepared.html)
- [Phase 2.3: Failure Mode Analysis](../phase-2.3-failure-modes.md)
```

### Step 4: Manual Refresh Runbook

**Create**: `docs/operations/runbooks/02-refresh-operations/manual-refresh.md`

```markdown
# Manual Refresh Runbook

## Purpose
Manually trigger refresh of individual TVIEWs or all TVIEWs.

## When to Use
- After bulk data changes to backing table
- When refresh is stale (> 1 hour)
- After unplanned PostgreSQL restart
- When automated refresh fails

## Prerequisites
- psql access to database
- CONNECT privilege on target database

## Single TVIEW Refresh

### Basic Refresh
```sql
-- Refresh single TVIEW
SELECT pg_tviews_refresh('entity_name');

-- Wait for completion and check success
SELECT * FROM pg_tviews_metadata WHERE entity_name = 'entity_name';
```

### With Error Handling
```sql
DO $$
DECLARE
  result TEXT;
  error_msg TEXT;
BEGIN
  PERFORM pg_tviews_refresh('entity_name');
  RAISE NOTICE 'Refresh completed successfully';
EXCEPTION WHEN OTHERS THEN
  GET STACKED DIAGNOSTICS error_msg = MESSAGE_TEXT;
  RAISE NOTICE 'Refresh failed: %', error_msg;
  -- Log error somewhere
END $$;
```

### Force Refresh
```sql
-- Refresh regardless of timestamps
SELECT pg_tviews_refresh('entity_name', force => true);

-- Wait and verify
SELECT COUNT(*) FROM entity_name;
SELECT COUNT(*) FROM backing_table;
-- Counts should match
```

## Refresh All TVIEWs

### Sequential Refresh (safe, slow)
```sql
DO $$
DECLARE
  rec RECORD;
  count INT := 0;
BEGIN
  FOR rec IN SELECT entity_name FROM pg_tviews_metadata
             ORDER BY refresh_priority DESC LOOP
    RAISE NOTICE 'Refreshing % (%/%)', rec.entity_name, count + 1,
                 (SELECT COUNT(*) FROM pg_tviews_metadata);
    PERFORM pg_tviews_refresh(rec.entity_name);
    count := count + 1;
  END LOOP;
  RAISE NOTICE 'Completed refreshing % TVIEWs', count;
END $$;
```

### Parallel Refresh (faster, requires testing)
```bash
#!/bin/bash
# Refresh all TVIEWs in parallel (up to 4 concurrent)

psql -tAc "SELECT entity_name FROM pg_tviews_metadata" | \
  xargs -P 4 -I {} bash -c '
    echo "Refreshing {}"
    psql -c "SELECT pg_tviews_refresh(\"{}\");"
  '

echo "All refreshes completed"
```

## Monitoring Refresh Progress

### During Refresh
```sql
-- Watch active refresh transactions
SELECT pid, query_start, state, query
FROM pg_stat_activity
WHERE query ILIKE '%tview%'
ORDER BY query_start DESC;

-- Check queue (should be decreasing)
SELECT COUNT(*) FROM pg_tviews_get_queue();
```

### Refresh Performance
```sql
-- How long did last refresh take?
SELECT entity_name,
  EXTRACT(EPOCH FROM (now() - last_refresh_time)) as seconds_ago,
  last_refresh_duration_ms
FROM pg_tviews_metadata
WHERE entity_name = 'entity_name';

-- If duration > 10 seconds: may need optimization (see troubleshooting)
```

## Troubleshooting

### Refresh Fails with Error
```sql
-- See the error
SELECT last_error FROM pg_tviews_metadata WHERE entity_name = 'entity_name';

-- Common errors:
-- - "relation does not exist": Check backing table exists
-- - "out of memory": Increase work_mem, try incremental refresh
-- - "timeout": Refresh taking too long, check large data changes
```

### Refresh Takes Very Long
```sql
-- Check what's happening
SELECT * FROM pg_stat_activity WHERE query ILIKE '%entity_name%';

-- Check data size
SELECT pg_size_pretty(pg_total_relation_size('backing_table'));
SELECT COUNT(*) FROM backing_table;

-- May need to:
-- 1. Cancel and try incremental refresh
-- 2. Increase available memory
-- 3. Add indexes to backing table
```

### Timeout During Refresh
```sql
-- Increase timeout for current session
SET statement_timeout = 60000;  -- 60 seconds
SELECT pg_tviews_refresh('entity_name');
RESET statement_timeout;
```

## Success Criteria
- ✅ Refresh completes without error
- ✅ TVIEW row count matches backing table
- ✅ No orphaned queue entries
- ✅ Refresh duration < 10 seconds

## Safety Notes
- ✅ Refresh is always safe - no data loss
- ✅ Can refresh same TVIEW multiple times
- ✅ Safe during active queries on TVIEW
- ❌ Don't use force => true on very large TVIEWs (> 100M rows)

## References
- [Performance Troubleshooting](./refresh-troubleshooting.md)
- [Batch Refresh](./batch-refresh.md)
```

### Step 5: Emergency Procedures Runbook

**Create**: `docs/operations/runbooks/04-incident-response/emergency-procedures.md`

```markdown
# Emergency Procedures Runbook

## Purpose
Quick procedures for critical incidents affecting pg_tviews.

## When to Use
- TVIEW refreshes completely stopped
- Data corruption detected
- Production incident requiring immediate action
- Queue stuck with thousands of entries

## Status: EMERGENCY
**Expected Duration**: 5-15 minutes
**Impact**: TVIEWs will be read-only until restored

---

## EMERGENCY 1: Disable All TVIEW Triggers

**Use When**: Refresh loop causing cascading failures

**Effect**: TVIEWs become static snapshots (no auto-refresh)

### Execute

```sql
-- Step 1: Disable all TVIEW refresh triggers
DO $$
DECLARE
  rec RECORD;
  count INT := 0;
BEGIN
  FOR rec IN
    SELECT DISTINCT trigger_name, event_object_table
    FROM information_schema.triggers
    WHERE trigger_name LIKE 'pg_tviews_%'
  LOOP
    EXECUTE format('ALTER TABLE %I DISABLE TRIGGER %I',
                  rec.event_object_table, rec.trigger_name);
    count := count + 1;
  END LOOP;
  RAISE NOTICE 'Disabled % triggers', count;
END $$;

-- Step 2: Verify triggers disabled
SELECT trigger_name, event_object_table, is_enabled
FROM information_schema.triggers
WHERE trigger_name LIKE 'pg_tviews_%'
AND is_enabled = true;
-- Should return NO ROWS if all disabled

-- Step 3: Notify team
-- TVIEWS ARE NOW READ-ONLY - DO NOT RUN APPLICATIONS EXPECTING AUTO-REFRESH
```

### Restore

```sql
-- When issue is fixed, re-enable triggers
DO $$
DECLARE
  rec RECORD;
  count INT := 0;
BEGIN
  FOR rec IN
    SELECT DISTINCT trigger_name, event_object_table
    FROM information_schema.triggers
    WHERE trigger_name LIKE 'pg_tviews_%'
  LOOP
    EXECUTE format('ALTER TABLE %I ENABLE TRIGGER %I',
                  rec.event_object_table, rec.trigger_name);
    count := count + 1;
  END LOOP;
  RAISE NOTICE 'Enabled % triggers', count;
END $$;

-- Step 2: Refresh all TVIEWs to get current data
DO $$
DECLARE
  rec RECORD;
BEGIN
  FOR rec IN SELECT entity_name FROM pg_tviews_metadata LOOP
    PERFORM pg_tviews_refresh(rec.entity_name, force => true);
  END LOOP;
END $$;

-- Step 3: Verify
SELECT COUNT(*) as tview_count FROM pg_tviews_metadata;
SELECT COUNT(*) FROM pg_tviews_get_queue();  -- Should be small
```

---

## EMERGENCY 2: Clear Stuck Queue

**Use When**: Queue has 1000+ entries stuck for > 30 minutes

**Effect**: Clears queue, may lose some pending refreshes

### Execute

```sql
-- Step 1: Identify what's stuck
SELECT entity_name, COUNT(*) as count,
  MAX(entry_time) as oldest
FROM pg_tviews_get_queue()
GROUP BY entity_name
ORDER BY count DESC;

-- Step 2: Stop applications (they won't get updates)
-- Notify: All TVIEW updates are paused

-- Step 3: Disable triggers (stops new queue entries)
DO $$
DECLARE
  rec RECORD;
BEGIN
  FOR rec IN
    SELECT DISTINCT trigger_name, event_object_table
    FROM information_schema.triggers
    WHERE trigger_name LIKE 'pg_tviews_%'
  LOOP
    EXECUTE format('ALTER TABLE %I DISABLE TRIGGER %I',
                  rec.event_object_table, rec.trigger_name);
  END LOOP;
END $$;

-- Step 4: Wait for in-flight transactions to complete
SELECT pg_sleep(60);

-- Step 5: Check queue again
SELECT COUNT(*) FROM pg_tviews_get_queue();

-- Step 6: If queue still has entries, TRUNCATE internal queue table
-- WARNING: This is destructive, only do after all refreshes should be done
-- TRUNCATE pg_tviews_internal.queue;

-- Step 7: Re-enable triggers
DO $$
DECLARE
  rec RECORD;
BEGIN
  FOR rec IN
    SELECT DISTINCT trigger_name, event_object_table
    FROM information_schema.triggers
    WHERE trigger_name LIKE 'pg_tviews_%'
  LOOP
    EXECUTE format('ALTER TABLE %I ENABLE TRIGGER %I',
                  rec.event_object_table, rec.trigger_name);
  END LOOP;
END $$;

-- Step 8: Manually refresh all TVIEWs
DO $$
DECLARE
  rec RECORD;
  count INT := 0;
BEGIN
  FOR rec IN SELECT entity_name FROM pg_tviews_metadata LOOP
    PERFORM pg_tviews_refresh(rec.entity_name, force => true);
    count := count + 1;
  END LOOP;
  RAISE NOTICE 'Refreshed % TVIEWs', count;
END $$;

-- Step 9: Verify
SELECT COUNT(*) FROM pg_tviews_get_queue();  -- Should be 0
```

---

## EMERGENCY 3: PostgreSQL Running Out of Memory

**Use When**: PostgreSQL process consuming excessive memory, server becoming unresponsive

**Effect**: Stops large TVIEW operations to free memory

### Execute

```sql
-- Step 1: Check memory pressure
SELECT
  (SELECT COUNT(*) FROM pg_stat_activity) as connections,
  (SELECT SUM(heap_blks_read) FROM pg_statio_user_tables) as cache_hits,
  pg_database_size(current_database()) as db_size;

-- Step 2: Kill large queries
SELECT pid, usename, state, query_start, query
FROM pg_stat_activity
WHERE query ILIKE '%tview%'
AND (now() - query_start) > interval '1 minute';

-- Step 3: Kill old/stuck queries
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE pid <> pg_backend_pid()
AND state = 'active'
AND (now() - query_start) > interval '5 minutes';

-- Step 4: Reduce work_mem for session
SET work_mem = '64MB';  -- Reduce from default

-- Step 5: Disable triggers to stop new operations
-- (See EMERGENCY 1)

-- Step 6: Wait for memory to recover
SELECT pg_sleep(30);

-- Step 7: Check memory
SELECT
  COUNT(*) as active_queries,
  SUM(extract(epoch from (now() - query_start))) as total_query_time
FROM pg_stat_activity
WHERE state != 'idle';
```

---

## EMERGENCY 4: Connection Limit Reached

**Use When**: Applications can't connect due to max_connections limit

**Effect**: Forcefully closes idle and old connections

### Execute

```sql
-- Step 1: See what's using connections
SELECT usename, COUNT(*) as count, state
FROM pg_stat_activity
GROUP BY usename, state
ORDER BY count DESC;

-- Step 2: Kill idle connections
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE state = 'idle'
AND (now() - state_change) > interval '10 minutes'
AND pid <> pg_backend_pid();

-- Step 3: Kill long-running non-critical queries
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE usename != 'postgres'
AND (now() - query_start) > interval '30 minutes'
AND pid <> pg_backend_pid();

-- Step 4: Verify
SELECT current_setting('max_connections') as limit,
  COUNT(*) as current
FROM pg_stat_activity
GROUP BY 1;
```

---

## Post-Emergency Actions

After any emergency procedure:

1. **Document**: Record timeline and root cause
2. **Verify**: Run health check
   ```bash
   docs/operations/runbooks/01-health-monitoring/tview-health-check.md
   ```
3. **Restore**: Manual refresh all TVIEWs if needed
4. **Monitor**: Watch closely for 1 hour
5. **Investigate**: Root cause analysis
6. **Prevent**: Implement safeguards

## Prevention Checklist

- [ ] Set up monitoring alerts (queue > 100)
- [ ] Set up monitoring alerts (memory > 80%)
- [ ] Set up monitoring alerts (connections > 90%)
- [ ] Document emergency contacts
- [ ] Test emergency procedures monthly
- [ ] Have runbook printed and accessible

## Escalation Path

1. **Minor Issue** (5 min to resolve)
   - On-call engineer

2. **Major Issue** (> 30 min to resolve)
   - Escalate to database team
   - Page on-call architect

3. **Critical Issue** (data loss risk)
   - Page all on-call staff
   - Call incident commander
   - Prepare customer communication

## References
- [Health Check](./tview-health-check.md)
- [Phase 2.3: Failure Modes](../../phase-2.3-failure-modes.md)
```

### Step 6: Create Supporting SQL Scripts

**Create**: `docs/operations/runbooks/scripts/health-check.sql`

```sql
-- health-check.sql
-- Quick health check for pg_tviews
-- Usage: psql -f health-check.sql

\timing on
\set QUIET off

-- Set session parameters
SET search_path = public, pg_tviews;

TITLE 'PG_TVIEWS HEALTH CHECK';
TITLE '================================';

-- Check 1: Extension installed
SELECT 'Extension Status' as check_name;
SELECT 'pg_tviews version: ' || pg_tviews_version() as result;
SELECT '';

-- Check 2: Metadata health
SELECT 'Metadata Tables' as check_name;
SELECT 'Total TVIEWs defined: ' || COUNT(*) FROM pg_tviews_metadata;
SELECT 'TVIEWs with errors: ' || COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL;
SELECT '';

-- Check 3: Queue status
SELECT 'Queue Status' as check_name;
WITH queue_stats AS (
  SELECT COUNT(*) as size,
    MAX(entry_time) as oldest_entry,
    EXTRACT(EPOCH FROM (now() - MIN(entry_time))) as max_age_sec
  FROM pg_tviews_get_queue()
)
SELECT 'Queue size: ' || size FROM queue_stats;
SELECT 'Queue age (sec): ' || COALESCE(CAST(max_age_sec as INT), 0) FROM queue_stats;
SELECT '';

-- Check 4: TVIEW status
SELECT 'TVIEW Status' as check_name;
WITH status AS (
  SELECT
    CASE
      WHEN backing_table_missing THEN '❌'
      WHEN last_error IS NOT NULL THEN '⚠️'
      WHEN stale THEN '⚠️'
      ELSE '✅'
    END as status,
    COUNT(*) as count
  FROM (
    SELECT
      entity_name,
      (SELECT COUNT(*) FROM information_schema.tables
       WHERE table_name = backing_table_name) = 0 as backing_table_missing,
      last_error,
      (now() - last_refresh_time) > interval '1 hour' as stale
    FROM pg_tviews_metadata
  ) sub
  GROUP BY 1
)
SELECT 'Status ' || status || ': ' || count FROM status;
SELECT '';

-- Check 5: Active transactions
SELECT 'Active Operations' as check_name;
SELECT 'Active PG_TVIEWS queries: ' || COUNT(*)
FROM pg_stat_activity
WHERE query ILIKE '%tview%' AND state != 'idle';
SELECT 'Active connections: ' || COUNT(*) FROM pg_stat_activity;
SELECT '';

TITLE 'END HEALTH CHECK';
```

---

## Verification Commands

```bash
# Verify runbook structure exists
test -d docs/operations/runbooks/01-health-monitoring
test -d docs/operations/runbooks/02-refresh-operations
test -d docs/operations/runbooks/03-maintenance
test -d docs/operations/runbooks/04-incident-response
test -d docs/operations/runbooks/scripts

# Test SQL scripts syntax
psql -1f docs/operations/runbooks/scripts/health-check.sql --dry-run

# Verify all runbooks are readable
for f in docs/operations/runbooks/**/*.md; do
  wc -l "$f"
done
```

---

## Acceptance Criteria

- [ ] All 4 runbook categories created with clear procedures
- [ ] Each runbook has clear "When to Use" section
- [ ] Each runbook includes executable SQL/bash examples
- [ ] Health check runbook verified with test database
- [ ] Queue management procedures tested
- [ ] Refresh procedures tested and working
- [ ] Emergency procedures documented and tested
- [ ] All runbooks use consistent formatting
- [ ] Supporting SQL scripts in scripts/ directory
- [ ] No hardcoded database names (use parameterized versions)

---

## DO NOT

- ❌ Create runbooks for untested procedures
- ❌ Include outdated command syntax
- ❌ Skip verification examples
- ❌ Write procedures that require manual parsing
- ❌ Forget error handling in complex procedures
- ❌ Leave ambiguous next steps ("contact admin")
- ❌ Create procedures without rollback/restore steps

---

## Rollback Plan

No rollback needed - this phase only adds documentation.

If runbooks need updates after deployment:
```bash
# Update in place
git add docs/operations/runbooks/
git commit -m "docs(ops): Update runbook [VERSION]"
```

---

## Next Steps

After completion:
- Commit with message: `docs(ops): Add comprehensive operations runbooks [PHASE5.1]`
- Test all runbooks with sample database
- Have ops team review for accuracy
- Proceed to **Phase 5.2: Upgrade & Migration Guides**

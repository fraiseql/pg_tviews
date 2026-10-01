# API Reference

Complete reference for all public PostgreSQL functions exposed by pg_tviews.

**Version**: 0.1.0-beta.1 • **Last Updated**: December 11, 2025

## Overview

pg_tviews provides a comprehensive set of functions for managing transactional materialized views. Functions are organized by category for easy navigation.

## Function Categories

- [Extension Management](#extension-management) - Version info, feature detection
- [DDL Operations](#ddl-operations) - TVIEW creation and management
- [Queue Management](#queue-management) - Monitor refresh queues
- [Debugging & Introspection](#debugging--introspection) - Analyze queries, debug issues
- [Two-Phase Commit (2PC)](#two-phase-commit-2pc) - Distributed transaction support
- [Manual Operations](#manual-operations) - Force refresh operations

## Extension Management

### pg_tviews_version()

**Signature**:
```sql
pg_tviews_version() RETURNS TEXT
```

**Description**:
Returns the version string of the pg_tviews extension.

**Parameters**:
- None

**Returns**:
- `TEXT`: Version string in format "major.minor.patch-suffix"

**Example**:
```sql
SELECT pg_tviews_version();
-- Returns: '0.1.0-beta.1'
```

**Notes**:
- Useful for verifying extension installation
- Version follows semantic versioning

### pg_tviews_check_jsonb_delta()

**Signature**:
```sql
pg_tviews_check_jsonb_delta() RETURNS BOOLEAN
```

**Description**:
Checks if the optional `jsonb_delta` extension is available at runtime. This extension provides performance optimizations for JSONB array operations.

**Parameters**:
- None

**Returns**:
- `BOOLEAN`: `true` if `jsonb_delta` is installed, `false` otherwise

**Example**:
```sql
SELECT pg_tviews_check_jsonb_delta();
-- Returns: true (if jsonb_delta is installed)
```

**Notes**:
- Result is cached after first check for performance
- `jsonb_delta` provides 1.5-3× faster JSONB updates when available

## DDL Operations

### pg_tviews_create() - Programmatic TVIEW Creation

**Signature**:
```sql
pg_tviews_create(tview_name TEXT, select_sql TEXT) RETURNS TEXT
```

**Description**:
Creates a new transactional view (TVIEW) from a SELECT statement. This is the **primary method** for creating TVIEWs. The TVIEW will automatically maintain consistency with its base tables through triggers.

**Parameters**:
- `tview_name` (TEXT): Name of the TVIEW (must follow `tv_*` naming convention)
- `select_sql` (TEXT): SELECT statement defining the view

**Returns**:
- `TEXT`: Success message or error description

**Example**:
```sql
SELECT pg_tviews_create('tv_user_posts',
    'SELECT u.pk_user, u.id, u.name, p.title
     FROM tb_user u
     JOIN tb_post p ON u.pk_user = p.fk_user');
-- Returns: 'TVIEW ''tv_user_posts'' created successfully'
```

**Notes**:
- TVIEW name must start with `tv_` (enforced)
- SELECT statement must be valid and reference existing tables
- Triggers are automatically created on base tables
- Alternative DDL syntax (`CREATE TABLE tv_* AS SELECT`) also available

**Indexes created on the TVIEW table** (both creation paths):

| Index | Columns | Purpose |
|---|---|---|
| `<tv>_pkey` (primary key) | `pk_<entity>` | row identity, refresh upserts |
| `idx_<tv>_id` | `id` | lookup by public UUID |
| `idx_<tv>_<uuid_fk>` | each UUID FK column | filtering by related public id |
| `idx_<tv>_<fk>_<pk>` **(required)** | `(fk_<x>, pk_<entity>)` per integer FK | cascade propagation lookup (`WHERE fk_<x> = ANY(…)`); without it every cascade step scans the whole TVIEW |
| `idx_<tv>_data_gin` | `data` (GIN) | **only with `pg_tviews.data_gin_index = on`**: top-level containment queries; blocks HOT on every refresh |

TVIEW tables are created `WITH (fillfactor = pg_tviews.fillfactor)` (default 85) so
refreshes can stay heap-only; see [HOT Updates and TVIEW Storage](../operations/hot-updates.md).

The propagation indexes are required for cascade performance: don't drop them.
Names longer than 63 bytes are shortened deterministically with a hash suffix.
TVIEWs created before these indexes existed can be upgraded with
[`pg_tviews_ensure_propagation_indexes()`](#pg_tviews_ensure_propagation_indexes).

### pg_tviews_create_or_replace()

**Signature**:
```sql
tviews.pg_tviews_create_or_replace(tview_name TEXT, query TEXT, options JSONB DEFAULT '{}')
RETURNS TEXT  -- 'created' | 'unchanged' | 'altered' | 'replaced' | 'rebuilt'
```

Creates a TVIEW, or brings an existing one to `query` and `options` with the smallest
change. Options: `logged`, `fillfactor`, `data_gin_index`, `group_keys`. See
[the contract for tools](read-contract.md) for the rules.

### pg_tviews_reregister() / pg_tviews_reregister_all()

**Signatures**:
```sql
tviews.pg_tviews_reregister(tview_name TEXT) RETURNS TEXT
tviews.pg_tviews_reregister_all(strict BOOLEAN DEFAULT false)
    RETURNS TABLE (entity TEXT, status TEXT)
```

Re-derive TVIEWs' metadata and base-table triggers from their stored definitions with
the installed release's analysis, without touching their rows, and clear
`needs_reregister`. Run `reregister_all()` after an upgrade when the release notes say
so or `pg_tviews_health_check()` reports TVIEWs to re-register.

### pg_tviews_drop()

**Signature**:
```sql
pg_tviews_drop(tview_name TEXT, if_exists BOOLEAN DEFAULT false) RETURNS TEXT
```

**Description**:
Drops an existing transactional view and cleans up all associated metadata and triggers.

**Parameters**:
- `tview_name` (TEXT): Name of the TVIEW to drop
- `if_exists` (BOOLEAN, optional): If true, a missing TVIEW raises a NOTICE instead
  of an error, like `DROP TABLE IF EXISTS`

**Returns**:
- `TEXT`: what was done: `TVIEW '<name>' dropped successfully`, or with
  `if_exists`, `TVIEW '<name>' does not exist, nothing dropped`

**Example**:
```sql
SELECT pg_tviews_drop('tv_user_posts');
-- Returns: 'TVIEW ''tv_user_posts'' dropped successfully'

SELECT pg_tviews_drop('tv_nonexistent', true);
-- NOTICE:  TVIEW "tv_nonexistent" does not exist, skipping
-- Returns: 'TVIEW ''tv_nonexistent'' does not exist, nothing dropped'
```

**Notes**:
- Removes all triggers from base tables
- Cleans up metadata from `pg_tview_meta`
- Use `if_exists => true` for safe cleanup scripts

## Queue Management

### pg_tviews_queue_stats()

**Signature**:
```sql
pg_tviews_queue_stats() RETURNS JSONB
```

**Description**:
Returns comprehensive statistics about the current transaction's refresh queue operations.

**Parameters**:
- None

**Returns**:
- `JSONB`: Object containing queue metrics

**Example**:
```sql
SELECT pg_tviews_queue_stats();
```

Returns JSONB like:
```json
{
  "queue_size": 5,
  "total_refreshes": 23,
  "total_iterations": 2,
  "max_iterations": 3,
  "total_timing_ms": 45.2,
  "graph_cache_hit_rate": 0.85,
  "table_cache_hit_rate": 0.92,
  "graph_cache_hits": 12,
  "graph_cache_misses": 2,
  "table_cache_hits": 18,
  "table_cache_misses": 2,
  "direct_patch_captured": 0,
  "direct_patches_applied": 0,
  "direct_patch_fallbacks": 0,
  "view_recomputes": 23,
  "refresh_noop_skipped": 17
}
```

**Notes**:
- Safe for frequent monitoring (no performance impact)
- Queue and cache metrics are for the current transaction; the `direct_patch_*`,
  `view_recomputes` and `refresh_noop_skipped` counters are cumulative for the session
- `refresh_noop_skipped` counts refreshed rows that were **not** rewritten because
  their content was unchanged; `refresh_noop_skipped / view_recomputes` is the
  no-op share of recompute work
- Cache hit rates indicate optimization effectiveness

### pg_tviews_debug_queue()

**Signature**:
```sql
pg_tviews_debug_queue() RETURNS JSONB
```

**Description**:
Returns the current contents of the refresh queue for debugging purposes.

**Parameters**:
- None

**Returns**:
- `JSONB`: Array of queued refresh operations

**Example**:
```sql
SELECT pg_tviews_debug_queue();
```

Returns JSONB like:
```json
[
  {"entity": "user", "pk": 123},
  {"entity": "post", "pk": 456}
]
```

**Notes**:
- Shows entities and primary keys queued for refresh
- Thread-local state (safe for concurrent connections)
- Useful for debugging refresh cascades

## Debugging & Introspection

### pg_tviews_analyze_select()

**Signature**:
```sql
pg_tviews_analyze_select(sql TEXT) RETURNS JSONB
```

**Description**:
Analyzes a SELECT statement and returns inferred TVIEW schema information including column types and dependencies.

**Parameters**:
- `sql` (TEXT): SELECT statement to analyze

**Returns**:
- `JSONB`: Schema analysis results

**Example**:
```sql
SELECT pg_tviews_analyze_select('
    SELECT u.pk_user, u.id, u.name, p.title as post_title
    FROM tb_user u
    JOIN tb_post p ON u.pk_user = p.fk_user
');
```

Returns JSONB with schema information including column types and table dependencies.

**Notes**:
- Validates SQL syntax and table existence
- Infers column types from PostgreSQL catalog
- Identifies base table dependencies for trigger setup

### pg_tviews_infer_types()

**Signature**:
```sql
pg_tviews_infer_types(table_name TEXT, columns TEXT[]) RETURNS JSONB
```

**Description**:
Infers column types for specified columns in a table using PostgreSQL's type system.

**Parameters**:
- `table_name` (TEXT): Name of the table
- `columns` (TEXT[]): Array of column names to analyze

**Returns**:
- `JSONB`: Type information for each column

**Example**:
```sql
SELECT pg_tviews_infer_types('tb_user', ARRAY['id', 'name', 'created_at']);
```

Returns JSONB with type information for each requested column.

**Notes**:
- Uses PostgreSQL's pg_catalog for accurate type inference
- Handles user-defined types and domains
- Useful for TVIEW schema validation

## Two-Phase Commit (2PC)

pg_tviews refreshes the TVIEWs before `PREPARE TRANSACTION`, so the refresh writes
belong to the prepared transaction: `COMMIT PREPARED` applies them and
`ROLLBACK PREPARED` discards them, with the rest of the transaction. No extra call
is needed. Prepared transactions require `max_prepared_transactions > 0`.

## Manual Operations

### pg_tviews_cascade()

**Signature**:
```sql
pg_tviews_cascade(base_table_oid OID, pk_value BIGINT) RETURNS VOID
```

**Description**:
Manually triggers a cascade refresh for a specific entity and primary key value.

**Parameters**:
- `base_table_oid` (OID): PostgreSQL OID of the base table
- `pk_value` (BIGINT): Primary key value of the changed row

**Returns**:
- `VOID`

**Example**:
```sql
-- Force refresh for user ID 123
SELECT pg_tviews_cascade('tb_user'::regclass::oid, 123);
```

**Notes**:
- Bypasses normal transaction queue
- Should rarely be needed (triggers handle this automatically)
- Useful for manual data fixes or testing

### pg_tviews_insert()

**Signature**:
```sql
pg_tviews_insert(base_table_oid OID, pk_value BIGINT) RETURNS VOID
```

**Description**:
Manually triggers insert handling for a specific entity and primary key value.

**Parameters**:
- `base_table_oid` (OID): PostgreSQL OID of the base table
- `pk_value` (BIGINT): Primary key value of the inserted row

**Returns**:
- `VOID`

**Example**:
```sql
-- Manually process insert for user ID 456
SELECT pg_tviews_insert('tb_user'::regclass::oid, 456);
```

**Notes**:
- Currently delegates to `pg_tviews_cascade`
- Specialized handling for array relationships (future enhancement)

### pg_tviews_delete()

**Signature**:
```sql
pg_tviews_delete(base_table_oid OID, pk_value BIGINT) RETURNS VOID
```

**Description**:
Manually triggers delete handling for a specific entity and primary key value.

**Parameters**:
- `base_table_oid` (OID): PostgreSQL OID of the base table
- `pk_value` (BIGINT): Primary key value of the deleted row

**Returns**:
- `VOID`

**Example**:
```sql
-- Manually process delete for user ID 789
SELECT pg_tviews_delete('tb_user'::regclass::oid, 789);
```

**Notes**:
- Currently delegates to `pg_tviews_cascade`
- Specialized handling for array relationships (future enhancement)

### pg_tviews_convert_table()

Internal: called by the `pg_tviews_ddl_end` event trigger. A `CREATE TABLE tv_* AS`
is turned into a TVIEW by the ProcessUtility hook before PostgreSQL creates any table;
if a plain `tv_*` table reaches the event trigger anyway (pg_tviews not in
`shared_preload_libraries`), this function raises an error instead of leaving a table
that looks like a TVIEW. Create TVIEWs with `pg_tviews_create_or_replace()` or
`CREATE TABLE tv_<entity> AS SELECT …`.

### pg_tviews_health_check()

**Signature**:
```sql
pg_tviews_health_check() RETURNS TABLE (
    check_name TEXT,
    status TEXT,
    details TEXT
)
```

**Description**:
Performs comprehensive health checks on the pg_tviews installation and all TVIEWs.

**Parameters**:
- None

**Returns**:
- `check_name TEXT`: Name of the health check
- `status TEXT`: 'OK', 'WARNING', or 'ERROR'
- `details TEXT`: Detailed information about the check

**Example**:
```sql
-- Run full health check
SELECT * FROM pg_tviews_health_check();

-- Check only critical issues
SELECT * FROM pg_tviews_health_check()
WHERE status IN ('WARNING', 'ERROR');
```

**Notes**:
- Checks extension installation, metadata consistency, trigger health
- Run after upgrades or when troubleshooting issues
- Safe to run frequently (read-only operations)

### pg_tviews_ensure_propagation_indexes()

**Signature**:
```sql
pg_tviews_ensure_propagation_indexes(entity TEXT DEFAULT NULL, dry_run BOOLEAN DEFAULT false)
RETURNS SETOF TEXT
```

**Description**:
Creates the required `(fk_<x>, pk_<entity>)` propagation index for every integer
`fk_*` column of a TVIEW that has no index leading with that column. Any existing
index whose first column is the FK counts, so user-created indexes are respected.

**Parameters**:
- `entity` (TEXT): one entity (e.g. `'post'` for `tv_post`); `NULL` = all TVIEWs
- `dry_run` (BOOLEAN): report the DDL without running it

**Returns**:
- One `CREATE INDEX IF NOT EXISTS …` statement per missing index (executed unless `dry_run`)

**Example**:
```sql
-- After upgrading: add missing propagation indexes everywhere
SELECT * FROM pg_tviews_ensure_propagation_indexes();

-- Large TVIEWs: get the DDL, then run it by hand with CONCURRENTLY
SELECT replace(ddl, 'CREATE INDEX', 'CREATE INDEX CONCURRENTLY')
FROM pg_tviews_ensure_propagation_indexes(NULL, true) AS ddl;
```

**Notes**:
- Idempotent: a second call returns no rows
- Runs inside a transaction, so it takes a `SHARE` lock per index build; use the
  dry-run + `CONCURRENTLY` route on busy tables

## Views

### tviews.registry and tviews.contract_version()

The versioned read contract for tools: one row per TVIEW (schema, name, entity,
normalized query, base tables, options, `needs_reregister`). See
[the contract for tools](read-contract.md).

### pg_tviews_queue_realtime

**Description**:
Real-time view of the current refresh queue state.

**Columns**:
- `queue_size INTEGER`: Number of pending refresh operations
- `oldest_entry TIMESTAMPTZ`: When the oldest queue entry was created
- `newest_entry TIMESTAMPTZ`: When the newest queue entry was created

**Example**:
```sql
-- Monitor queue in real-time
SELECT * FROM pg_tviews_queue_realtime;

-- Alert on queue buildup
SELECT CASE
    WHEN queue_size > 1000 THEN 'CRITICAL'
    WHEN queue_size > 100 THEN 'WARNING'
    ELSE 'OK'
END as queue_status
FROM pg_tviews_queue_realtime;
```

**Notes**:
- Updated in real-time as operations are queued/dequeued
- Useful for monitoring and alerting
- Very fast (no table scans)

### pg_tviews_cache_stats

**Description**:
Statistics about internal caching performance.

**Columns**:
- `cache_name TEXT`: Name of the cache
- `entries INTEGER`: Number of cached entries
- `hit_rate NUMERIC`: Cache hit rate (0.0 to 1.0)
- `last_accessed TIMESTAMPTZ`: When cache was last accessed

**Example**:
```sql
-- Check cache performance
SELECT * FROM pg_tviews_cache_stats;

-- Monitor cache efficiency
SELECT
    cache_name,
    hit_rate * 100 as hit_percentage,
    CASE
        WHEN hit_rate > 0.9 THEN 'EXCELLENT'
        WHEN hit_rate > 0.7 THEN 'GOOD'
        ELSE 'NEEDS_ATTENTION'
    END as performance
FROM pg_tviews_cache_stats;
```

**Notes**:
- Tracks prepared statements and graph cache performance
- Useful for performance tuning
- Reset on extension reload

## Common Usage Patterns

### Check Extension Status
```sql
-- Verify extension is installed
SELECT pg_tviews_version();

-- Check for optional performance extension
SELECT pg_tviews_check_jsonb_delta();
```

### Monitor Queue Activity
```sql
-- Get current queue statistics
SELECT pg_tviews_queue_stats();

-- View queued refresh operations
SELECT pg_tviews_debug_queue();
```

### Debug View Definitions
```sql
-- Analyze SELECT for TVIEW compatibility
SELECT pg_tviews_analyze_select('
    SELECT p.pk_post, p.id, p.title, u.name as author
    FROM tb_post p JOIN tb_user u ON p.fk_user = u.pk_user
');

-- Check inferred column types
SELECT pg_tviews_infer_types('tb_user', ARRAY['id', 'name']);
```

### Two-Phase Commit Workflow
```sql
BEGIN;
INSERT INTO tb_post (fk_user, title) VALUES (1, 'New Post');
PREPARE TRANSACTION 'txn-123';   -- TVIEWs refreshed as part of the transaction

COMMIT PREPARED 'txn-123';       -- or ROLLBACK PREPARED 'txn-123'
```

### Manual Refresh Operations
```sql
-- Force refresh a specific entity
SELECT pg_tviews_cascade('tb_user'::regclass::oid, 123);

-- Process after manual data correction
SELECT pg_tviews_insert('tb_post'::regclass::oid, 456);
```

## Important Notes

### Performance Considerations
- `pg_tviews_debug_queue()` reads thread-local state, no performance impact
- `pg_tviews_queue_stats()` is fast, safe for frequent monitoring
- Manual operations (`pg_tviews_cascade`, etc.) bypass transaction queue

### Common Pitfalls
- Don't use manual operations in triggers (causes recursion)
- `pg_tviews_analyze_select()` doesn't validate table existence
- DDL operations require appropriate permissions

### Thread Safety
- Queue functions operate on thread-local state
- Safe for concurrent use across connections
- Each connection has isolated queue state

## Troubleshooting

### Function Not Found
```sql
ERROR:  function pg_tviews_version() does not exist
```
**Solution**: Extension not installed. Run `CREATE EXTENSION pg_tviews;`

### Permission Denied
```sql
ERROR:  must be owner of TVIEW tv_post
```
**Solution**: replacing, dropping or re-registering a TVIEW requires owning its
`tv_*` table (or being a member of the owning role, or the extension's owner).

### Invalid TVIEW Name
```sql
ERROR: TVIEW name must follow tv_* convention
```
**Solution**: Use names like `tv_user`, `tv_post`, etc.

## See Also

- [Monitoring Guide](../operations/monitoring.md)
- [Troubleshooting Guide](../operations/troubleshooting.md)
- [FraiseQL Integration Guide](../getting-started/fraiseql-integration.md)
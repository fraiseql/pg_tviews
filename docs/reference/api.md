# API Reference

Complete reference for all public PostgreSQL functions exposed by pg_tviews.

**Version**: 0.1.0-beta.1 • **Last Updated**: December 11, 2025

## Overview

pg_tviews provides a comprehensive set of functions for managing transactional materialized views. Functions are organized by category for easy navigation.

## Function Categories

- [Extension Management](#extension-management) - Version info, feature detection
- [DDL Operations](#ddl-operations) - TVIEW creation and management
- [Queue Management](#queue-management) - Monitor refresh queues
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
change. Options: `logged`, `fillfactor`, `data_gin_index`, `group_keys`,
`uncascaded_policy`. See
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

## Two-Phase Commit (2PC)

pg_tviews refreshes the TVIEWs before `PREPARE TRANSACTION`, so the refresh writes
belong to the prepared transaction: `COMMIT PREPARED` applies them and
`ROLLBACK PREPARED` discards them, with the rest of the transaction. No extra call
is needed. Prepared transactions require `max_prepared_transactions > 0`.

## Manual Operations

### pg_tviews_cascade()

Low-level. The triggers already do this on every write; use it to repair a TVIEW
after a change the triggers did not see (`session_replication_role = replica`,
triggers disabled), or prefer `pg_tviews_refresh(entity)`, which rebuilds it.

**Signature**:
```sql
pg_tviews_cascade(base_table_oid OID, pk_value BIGINT) RETURNS VOID
```

**Description**:
Queues a refresh of the TVIEW rows that read the row `pk_value` of
`base_table_oid`. Outside a transaction block it then refreshes them before
returning. Inside one, the refresh stays queued and runs with the transaction's
next flush: the next statement that writes a TVIEW's table, `COMMIT`, or
`PREPARE TRANSACTION`.

**Parameters**:
- `base_table_oid` (OID): PostgreSQL OID of the base table
- `pk_value` (BIGINT): primary key value of the changed row

**Returns**:
- `VOID`

**Example**:
```sql
-- The row pk_user = 123 changed while the triggers were off
SELECT pg_tviews_cascade('tb_user'::regclass::oid, 123);
```

**Notes**:
- The rows are found by the naming convention: `pk_<entity>` for the TVIEW's own
  table, `fk_<entity>` columns for the tables it reads. A table whose rows map to
  keys any other way is not covered; use `pg_tviews_refresh(entity)`.
- Refresh work still queued when a transaction commits is dropped with a WARNING.

### pg_tviews_insert()

**Signature**:
```sql
pg_tviews_insert(base_table_oid OID, pk_value BIGINT) RETURNS VOID
```

Same as `pg_tviews_cascade()`, for an inserted row. Low-level.

**Example**:
```sql
SELECT pg_tviews_insert('tb_user'::regclass::oid, 456);
```

### pg_tviews_delete()

**Signature**:
```sql
pg_tviews_delete(base_table_oid OID, pk_value BIGINT) RETURNS VOID
```

Same as `pg_tviews_cascade()`, for a deleted row. Low-level.

**Example**:
```sql
SELECT pg_tviews_delete('tb_user'::regclass::oid, 789);
```

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
    status TEXT,
    component TEXT,
    message TEXT,
    severity TEXT
)
```

**Description**:
Performs comprehensive health checks on the pg_tviews installation and all TVIEWs.

**Parameters**:
- None

**Returns** one row per check:
- `status TEXT`: `OK`, `WARNING` or `ERROR`
- `component TEXT`: what was checked: `extension`, `jsonb_delta`, `catalog`,
  `metadata`, `reregister`, `triggers`, `tviews`
- `message TEXT`: what the check found
- `severity TEXT`: `info`, `warning` or `error`

**Example**:
```sql
-- Run full health check
SELECT * FROM pg_tviews_health_check();

-- Check only critical issues
SELECT component, message FROM pg_tviews_health_check()
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

## Refresh and Repair

### pg_tviews_refresh()

```sql
pg_tviews_refresh(entity TEXT) RETURNS VOID
```

Rebuilds `tv_<entity>` from its view, then every TVIEW whose view reads it, directly
or through others, dependencies first: the repair for a TVIEW left stale by a change
its triggers did not see. Each rebuild is a `TRUNCATE` and an `INSERT … SELECT`,
holding an `ACCESS EXCLUSIVE` lock on that TVIEW until the transaction ends. Like
`REFRESH MATERIALIZED VIEW`, it requires owning `tv_<entity>` (being a member of its
owner's role) or the extension, and every TVIEW is rebuilt as its owner: a function
the backing view calls never runs with the caller's privileges.

```sql
SELECT pg_tviews_refresh('user');   -- tv_user, then tv_post (embeds user), tv_feed (embeds post)
```

### pg_tviews_refresh_time_dependent()

```sql
pg_tviews_refresh_time_dependent(tview TEXT DEFAULT NULL) RETURNS SETOF TEXT
```

Brings the TVIEWs whose definitions read the current time (`registry.time_dependent`)
up to date: `tview`, or every such TVIEW the caller owns. Each is refreshed in full
through the flush, as a write to a `full_refresh` table refreshes it, so the TVIEWs
reading it follow; returns the TVIEWs refreshed, dependencies first. A named TVIEW
that reads no time is an error. Call it at the boundary the rows depend on, from
pg_cron or the application ([Time-dependent
TVIEWs](ddl.md#time-dependent-tviews)).

```sql
SELECT * FROM tviews.pg_tviews_refresh_time_dependent();
```

### pg_tviews_refresh_all() / pg_tviews_refresh_all_entities()

```sql
pg_tviews_refresh_all() RETURNS JSONB
pg_tviews_refresh_all_entities() RETURNS VOID
```

Rebuild every TVIEW once, dependencies first, each as its owner. `pg_tviews_refresh_all()`
returns `{"refreshed_count", "order", "duration_ms"}` and refuses to run while refresh
is suspended; `pg_tviews_refresh_all_entities()` reports the count as an INFO message.

### pg_tviews_show_cascade_path()

```sql
pg_tviews_show_cascade_path(entity TEXT)
    RETURNS TABLE(depth INTEGER, entity_name TEXT, depends_on TEXT)
```

`entity` itself at depth 0, then the TVIEWs that embed it, with their depth: what
`pg_tviews_refresh(entity)` rebuilds.

### pg_tviews_mapping_query()

```sql
pg_tviews_mapping_query(tview TEXT, base_table OID) RETURNS TEXT
```

The query that maps rows changed in `base_table` to keys of `tview`, NULL when
writes to the table do not map through a query of their own. See
[How a write finds the TVIEW rows to refresh](ddl.md#how-a-write-finds-the-tview-rows-to-refresh).

## Suspending Refresh

```sql
pg_tviews_suspend_triggers() RETURNS VOID
pg_tviews_resume_triggers() RETURNS VOID
pg_tviews_is_suspended() RETURNS BOOLEAN
pg_tviews_suspended_entities() RETURNS TEXT[]
```

`pg_tviews_suspend_triggers()` defers refreshes for a bulk load; calls nest.
When the outermost `pg_tviews_resume_triggers()` runs, every TVIEW changed while
suspended, and every TVIEW embedding one of them, is rebuilt. Suspension ends with
the transaction: an explicit `COMMIT` catches up the same way, an implicit commit
logs a WARNING naming the stale TVIEWs. `pg_tviews_suspended_entities()` lists the
TVIEWs changed so far. Run the whole pattern in one transaction:

```sql
BEGIN;
SELECT pg_tviews_suspend_triggers();
INSERT INTO tb_order (fk_user) SELECT 1 FROM generate_series(1, 10000);
SELECT pg_tviews_resume_triggers();
COMMIT;
```

## Change Reports

### pg_tviews_flush_and_report()

```sql
pg_tviews_flush_and_report(max_entities INTEGER DEFAULT 500,
                           include_data BOOLEAN DEFAULT true,
                           reset BOOLEAN DEFAULT true) RETURNS JSONB
```

Flushes the queue, then reports the TVIEW rows this transaction changed, for a
GraphQL cascade response: each entry carries its type name, `id` and (with
`include_data`) the row's `data`; a deleted entry is `{"__typename", "id"}`. Past
`max_entities` entries, or when the journal overflowed `pg_tviews.report_max_tracked`,
`truncated` is true and `invalidated_types` lists the types left out. With `reset`,
the next call reports only later changes. See
[GraphQL cascade](../user-guides/graphql-cascade.md).

### pg_tviews_set_typename()

```sql
pg_tviews_set_typename(entity TEXT, typename TEXT) RETURNS VOID
```

Sets the GraphQL type name `pg_tviews_flush_and_report()` reports for `entity`;
NULL resets it to the PascalCase of the entity.

## Aggregate TVIEWs

### pg_tviews_create_aggregate()

```sql
pg_tviews_create_aggregate(tview_name TEXT, select_sql TEXT, group_keys JSONB) RETURNS TEXT
```

Creates a TVIEW keyed on a group (one row per `GROUP BY` key). `group_keys` maps each
source table to the column holding the group key:

```sql
SELECT pg_tviews_create_aggregate('tv_user_summary', $$
    SELECT o.fk_user AS pk_user_summary, u.id, jsonb_build_object('orders', count(*)) AS data
    FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id $$,
    '{"tb_order": "fk_user", "tb_user": "pk_user"}');
```

See [Aggregate TVIEWs](../user-guides/aggregate-tviews.md).

## Storage, Replication and Recovery

```sql
pg_tviews_set_logged(entity TEXT, logged BOOLEAN) RETURNS VOID
pg_tviews_is_replica_readable(entity TEXT) RETURNS BOOLEAN
pg_tviews_replication_status()
    RETURNS TABLE(entity TEXT, persistence TEXT, replica_readable BOOLEAN,
                  is_empty BOOLEAN, needs_rebuild BOOLEAN)
pg_tviews_rebuild_all(only_empty BOOLEAN DEFAULT true) RETURNS TABLE(entity TEXT, rows BIGINT)
pg_tviews_recover_after_crash(entity_name TEXT) RETURNS BOOLEAN
pg_tviews_profile(p_entity TEXT DEFAULT NULL, fanout_warn BIGINT DEFAULT 1000) RETURNS TABLE(…)
pg_tviews_catalog_revision() RETURNS INTEGER
```

- `pg_tviews_set_logged` switches `tv_<entity>` to LOGGED (readable on standbys) or
  back to UNLOGGED; `ALTER TABLE … SET [UN]LOGGED` rewrites the table under an
  `ACCESS EXCLUSIVE` lock.
- `pg_tviews_is_replica_readable` is true for a LOGGED TVIEW, false for an UNLOGGED
  one, NULL for an unknown entity; `pg_tviews_replication_status` reports every
  TVIEW and is safe on a standby.
- `pg_tviews_rebuild_all` refills the UNLOGGED TVIEWs a crash restart, promotion or
  restore left empty (every TVIEW with `only_empty => false`), dependencies first.
  `pg_tviews_recover_after_crash` does it for one entity and returns whether it had
  to. Both read each backing view and fill each TVIEW as the TVIEW's owner.
- `pg_tviews_profile` is the per-TVIEW physical health report:
  see [profile.md](profile.md).
- `pg_tviews_catalog_revision` is the revision of the extension's catalog the
  library checks before it works on it.

See [Replication](../operations/replication.md).

## Internal Functions

Called by triggers, event triggers and restore; don't call them directly:
`pg_tviews_audit_write`, `pg_tviews_defines_view`, `pg_tviews_handle_dropped`,
`pg_tviews_invalidate_caches`, `pg_tviews_meta_changed`, `pg_tviews_meta_rebind`,
`pg_tviews_migrate_triggers`, `pg_tviews_rebind_cascade_paths`, and the trigger
functions `pg_tview_trigger_handler`, `pg_tview_flush_trigger`,
`pg_tview_delta_trigger`, `pg_tview_truncate_trigger`.
`pg_tviews_convert_existing_table` is deprecated and always raises an error: use
`pg_tviews_create()` or `CREATE TABLE tv_x AS SELECT …`.

## Views

### tviews.registry and tviews.contract_version()

The versioned read contract for tools: one row per TVIEW (schema, name, entity,
normalized query, base tables, options, `needs_reregister`). See
[the contract for tools](read-contract.md).

The queue and cache counters are per transaction and per backend; read them with
`pg_tviews_queue_stats()`. `pg_tviews_health_check()` and
`pg_tviews_performance_stats()` cover the server-wide picture.

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

### Two-Phase Commit Workflow
```sql
BEGIN;
INSERT INTO tb_post (fk_user, title) VALUES (1, 'New Post');
PREPARE TRANSACTION 'txn-123';   -- TVIEWs refreshed as part of the transaction

COMMIT PREPARED 'txn-123';       -- or ROLLBACK PREPARED 'txn-123'
```

### Manual Refresh Operations
```sql
-- Refresh the TVIEW rows that read one row of tb_user
SELECT pg_tviews_cascade('tb_user'::regclass::oid, 123);

-- Process after manual data correction
SELECT pg_tviews_insert('tb_post'::regclass::oid, 456);
```

## Important Notes

### Performance Considerations
- `pg_tviews_debug_queue()` reads thread-local state, no performance impact
- `pg_tviews_queue_stats()` is fast, safe for frequent monitoring
- Manual operations (`pg_tviews_cascade`, etc.) use the transaction queue; in autocommit they flush it before returning

### Common Pitfalls
- Don't use manual operations in triggers (causes recursion)
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
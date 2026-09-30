# pg_tviews SQL API Reference

## STABLE Functions

### pg_tviews_convert_existing_table(table_name TEXT)
**Status**: DEPRECATED (since v0.1.0-beta.18; removed in the next breaking release)
**Description**: Always raises an error. It could not run on PostgreSQL 18, and its design
replaced the table with a frozen snapshot view (no triggers, no refresh).
**Use instead**: `pg_tviews_create('entity', 'SELECT ...')` or
`CREATE TABLE tv_entity AS SELECT ...`.

---

### pg_tviews_version()
**Status**: STABLE (v0.1+)
**Description**: Get the pg_tviews extension version
**Returns**: TEXT (version string)
**Contract**: Always returns valid semver string

---

### pg_tviews_metadata(tview_name TEXT)
**Status**: STABLE (v0.1+)
**Description**: Retrieve TVIEW metadata and configuration
**Returns**: TABLE(
  entity_name TEXT,
  primary_key TEXT,
  created_at TIMESTAMP,
  last_refreshed TIMESTAMP,
  rows_cached INT
)
**Contract**: Schema guaranteed stable (may add optional columns)

---

### pg_tviews_health_check()
**Status**: STABLE (v0.1+)
**Description**: Check extension health and connectivity
**Returns**: TABLE with health metrics
**Contract**: Output format stable

---

## EVOLVING Functions

### pg_tviews_debug_queue()
**Status**: EVOLVING
**Description**: Inspect current refresh queue (debugging)
**Stability Target**: STABLE in v1.1
**Returns**: JSONB with queue contents

**Known Future Changes**:
- May restructure output format for performance
- Add additional diagnostic fields
- Change refresh order/priority algorithm

---

### pg_tviews_queue_stats()
**Status**: EVOLVING
**Description**: Get queue statistics
**Stability Target**: STABLE in v1.1
**Returns**: JSONB with statistics

---

## EXPERIMENTAL Functions

### pg_tviews_clear_queue()
**Status**: EXPERIMENTAL
**Description**: Force-clear refresh queue (advanced debugging only)
**Warning**: Can cause data inconsistency if used incorrectly
**Returns**: Success/error message

**This function may be removed or significantly changed**:
- Only use under guidance from pg_tviews team
- Not recommended for automated operations
- May be replaced with safer alternative

---

### pg_tviews_performance_stats()
**Status**: EXPERIMENTAL
**Description**: Get detailed performance statistics
**Warning**: Output format may change frequently
**Returns**: TABLE with performance metrics

---

### pg_tviews_create(tview_name TEXT, select_sql TEXT)
**Status**: EXPERIMENTAL
**Description**: Create a TVIEW from SQL (alternative to DDL)
**Warning**: Limited validation, use DDL syntax instead
**Returns**: Success/error message

---

### pg_tviews_drop(tview_name TEXT, if_exists BOOLEAN)
**Status**: EXPERIMENTAL
**Description**: Drop a TVIEW (alternative to DDL)
**Warning**: Limited validation, use DDL syntax instead
**Returns**: Success/error message

---

### pg_tviews_refresh(tview_name TEXT)
**Status**: EXPERIMENTAL
**Description**: Force refresh a TVIEW (benchmarking only)
**Warning**: Bypasses incremental refresh, use for testing only
**Returns**: Success/error message

---

### pg_tviews_commit_prepared(gid TEXT)
**Status**: EXPERIMENTAL
**Description**: Commit prepared 2PC transaction
**Warning**: Advanced usage, requires 2PC knowledge
**Returns**: Success/error

---

### pg_tviews_rollback_prepared(gid TEXT)
**Status**: EXPERIMENTAL
**Description**: Rollback prepared 2PC transaction
**Warning**: Advanced usage, requires 2PC knowledge
**Returns**: Success/error

---

## DEPRECATED Functions

*None currently, but example format:*

### pg_tviews_legacy_refresh_all() [DEPRECATED in 0.2]
**Status**: DEPRECATED (Remove in v1.0)
**Replacement**: `pg_tviews_refresh_all(filter_pattern TEXT DEFAULT '%')`
**Migration**: See [BREAKING_CHANGES_V2.0](../BREAKING_CHANGES_V2.0.md)
**Removal Date**: 2026-06-01

```sql
-- OLD (deprecated)
SELECT pg_tviews_legacy_refresh_all();

-- NEW (use instead)
SELECT pg_tviews_refresh_all('%');
```
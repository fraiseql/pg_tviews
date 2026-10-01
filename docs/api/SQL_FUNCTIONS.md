# pg_tviews SQL API Reference

## STABLE Functions

### pg_tviews_convert_existing_table(table_name TEXT)
**Status**: DEPRECATED (since v0.1.0-beta.18; removed in the next breaking release)
**Description**: Always raises an error. It could not run on PostgreSQL 18, and its design
replaced the table with a frozen snapshot view (no triggers, no refresh).
**Use instead**: `pg_tviews_create_or_replace('tv_entity', 'SELECT ...')` or
`CREATE TABLE tv_entity AS SELECT ...`.

---

### pg_tviews_version()
**Status**: STABLE (v0.1+)
**Description**: Get the pg_tviews extension version
**Returns**: TEXT (version string)
**Contract**: Always returns valid semver string

---

### tviews.registry and tviews.contract_version()
**Status**: STABLE, versioned by `contract_version()`
**Description**: One row per registered TVIEW (schema, name, entity, normalized query,
base tables, options, `needs_reregister`), and the version of that contract
**Contract**: [docs/reference/read-contract.md](../reference/read-contract.md)

---

### pg_tviews_create_or_replace(tview_name TEXT, query TEXT, options JSONB)
**Status**: STABLE, versioned by `contract_version()`
**Description**: Create a TVIEW, or bring an existing one to `query` and `options`
with the smallest change
**Returns**: `created`, `unchanged`, `altered`, `replaced` or `rebuilt`
**Contract**: [docs/reference/read-contract.md](../reference/read-contract.md)

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

## DEPRECATED Functions

- `pg_tviews_convert_existing_table()` (see above).

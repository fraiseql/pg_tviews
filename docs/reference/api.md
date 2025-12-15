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
- [jsonb_ivm Integration Functions](#jsonb_ivm-integration-functions-v02) - Enhanced JSONB operations (v0.2+)

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

### pg_tviews_check_jsonb_ivm()

**Signature**:
```sql
pg_tviews_check_jsonb_ivm() RETURNS BOOLEAN
```

**Description**:
Checks if the optional `jsonb_ivm` extension is available at runtime. This extension provides performance optimizations for JSONB array operations.

**Parameters**:
- None

**Returns**:
- `BOOLEAN`: `true` if `jsonb_ivm` is installed, `false` otherwise

**Example**:
```sql
SELECT pg_tviews_check_jsonb_ivm();
-- Returns: true (if jsonb_ivm is installed)
```

**Notes**:
- Result is cached after first check for performance
- `jsonb_ivm` provides 1.5-3× faster JSONB updates when available

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

### pg_tviews_drop()

**Signature**:
```sql
pg_tviews_drop(tview_name TEXT, if_exists BOOLEAN DEFAULT false) RETURNS TEXT
```

**Description**:
Drops an existing transactional view and cleans up all associated metadata and triggers.

**Parameters**:
- `tview_name` (TEXT): Name of the TVIEW to drop
- `if_exists` (BOOLEAN, optional): If true, don't error if TVIEW doesn't exist

**Returns**:
- `TEXT`: Success message or error description

**Example**:
```sql
SELECT pg_tviews_drop('tv_user_posts');
-- Returns: 'TVIEW ''tv_user_posts'' dropped successfully'

SELECT pg_tviews_drop('tv_nonexistent', true);
-- Returns: 'TVIEW ''tv_nonexistent'' dropped successfully' (no error)
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
  "table_cache_misses": 2
}
```

**Notes**:
- Safe for frequent monitoring (no performance impact)
- All metrics are for the current transaction only
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

### pg_tviews_commit_prepared()

**Signature**:
```sql
pg_tviews_commit_prepared(gid TEXT) RETURNS VOID
```

**Description**:
Commits a prepared transaction and processes any pending TVIEW refreshes that were queued during the transaction.

**Parameters**:
- `gid` (TEXT): Global transaction identifier of the prepared transaction

**Returns**:
- `VOID`

**Example**:
```sql
-- In another session/connection:
COMMIT PREPARED 'my-transaction-123';

-- Then commit the TVIEW refreshes:
SELECT pg_tviews_commit_prepared('my-transaction-123');
```

**Notes**:
- Must be called after `COMMIT PREPARED`
- Processes refreshes in a new transaction
- Required for distributed transaction support

### pg_tviews_rollback_prepared()

**Signature**:
```sql
pg_tviews_rollback_prepared(gid TEXT) RETURNS VOID
```

**Description**:
Rolls back a prepared transaction and cleans up any pending TVIEW refresh queues.

**Parameters**:
- `gid` (TEXT): Global transaction identifier of the prepared transaction

**Returns**:
- `VOID`

**Example**:
```sql
-- Rollback the prepared transaction:
ROLLBACK PREPARED 'my-transaction-123';

-- Clean up TVIEW queues:
SELECT pg_tviews_rollback_prepared('my-transaction-123');
```

**Notes**:
- Must be called after `ROLLBACK PREPARED`
- Discards pending refreshes without processing
- Required for proper cleanup in distributed transactions

### pg_tviews_recover_prepared_transactions()

**Signature**:
```sql
pg_tviews_recover_prepared_transactions() RETURNS TABLE(gid TEXT, queue_size INT, status TEXT)
```

**Description**:
Recovers orphaned prepared transactions that have pending TVIEW refreshes. Automatically commits transactions older than 1 hour.

**Parameters**:
- None

**Returns**:
- TABLE with columns:
  - `gid` (TEXT): Transaction identifier
  - `queue_size` (INT): Number of pending refreshes
  - `status` (TEXT): Recovery status ('processed' or 'error')

**Example**:
```sql
SELECT * FROM pg_tviews_recover_prepared_transactions();
```

**Notes**:
- Uses advisory locks to prevent concurrent recovery
- Only processes transactions older than 1 hour
- Useful for disaster recovery scenarios

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

**Signature**:
```sql
pg_tviews_convert_table(
    table_name TEXT,
    entity_name TEXT DEFAULT NULL
) RETURNS BOOLEAN
```

**Description**:
Converts an existing regular table to a TVIEW by analyzing its structure and creating the necessary metadata and triggers.

**Parameters**:
- `table_name TEXT`: Name of the existing table to convert (must start with `tv_`)
- `entity_name TEXT`: Optional entity name (defaults to table name without `tv_` prefix)

**Returns**:
- `BOOLEAN`: True if conversion successful

**Example**:
```sql
-- Convert existing tv_* table to TVIEW
SELECT pg_tviews_convert_table('tv_post', 'post');

-- Check conversion result
SELECT * FROM pg_tview_meta WHERE entity = 'post';
SELECT * FROM tv_post LIMIT 5;
```

**Notes**:
- Table must already be named `tv_<entity>` and have `pk_<entity>` and `data` columns
- Creates backing view and installs triggers
- Useful for migrating existing materialized views

### pg_tviews_install_stmt_triggers()

**Signature**:
```sql
pg_tviews_install_stmt_triggers() RETURNS INTEGER
```

**Description**:
Installs statement-level triggers on all base tables to dramatically improve bulk operation performance (100-500× faster).

**Parameters**:
- None

**Returns**:
- `INTEGER`: Number of triggers installed

**Example**:
```sql
-- Enable high-performance bulk operations
SELECT pg_tviews_install_stmt_triggers();
-- Returns: 5 (number of triggers installed)

-- Verify triggers are active
SELECT COUNT(*) FROM pg_trigger WHERE tgname LIKE '%tview%';
```

**Notes**:
- Processes entire statements at once using transition tables
- Reduces trigger overhead from N× to 1× per statement
- Essential for high-throughput applications

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

## Views

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

## jsonb_ivm Integration Functions (v0.2+)

### Helper Functions

#### extract_jsonb_id()

Extract ID field from JSONB data using optimized jsonb_ivm function.

**Rust Signature**: `pub fn extract_jsonb_id(data: &JsonB, id_key: &str) -> spi::Result<Option<String>>`

**SQL Usage**: Via Rust function calls

**Performance**: 5× faster than `data->>'id'`

**Example**:
```rust
let id = extract_jsonb_id(&data, "id")?;
```

#### check_array_element_exists()

Fast array element existence check.

**Performance**: 10× faster than jsonb_path_query

---

### Nested Path Updates

#### jsonb_delta_array_update_where_path()

Update nested fields in array elements using path notation.

**Signature**:
```sql
jsonb_delta_array_update_where_path(
    data JSONB,
    array_path TEXT[],
    match_key TEXT,
    match_value JSONB,
    update_path TEXT,
    update_value JSONB
) RETURNS JSONB
```

**Description**:
Updates a specific field in array elements that match a condition, using dot-notation paths.

**Parameters**:
- `data` (JSONB): The JSONB data to update
- `array_path` (TEXT[]): Path to the array (e.g., `ARRAY['items']`)
- `match_key` (TEXT): Key to match elements on (e.g., `'id'`)
- `match_value` (JSONB): Value to match elements against
- `update_path` (TEXT): Dot-notation path to update (e.g., `'product.name'`)
- `update_value` (JSONB): New value for the field

**Returns**:
- `JSONB`: Updated data

**Performance**: 2-3× faster than nested jsonb_set operations

**Example**:
```sql
-- Update product name in specific order item
UPDATE tv_orders SET data = jsonb_delta_array_update_where_path(
    data,
    ARRAY['items'],
    'id',
    '"item-123"'::jsonb,
    'product.name',
    '"Updated Product"'::jsonb
)
WHERE pk_order = 1;
```

---

### Batch Operations

#### jsonb_array_update_where_batch()

Bulk update multiple array elements in a single operation.

**Signature**:
```sql
jsonb_array_update_where_batch(
    data JSONB,
    array_path TEXT[],
    match_key TEXT,
    updates JSONB
) RETURNS JSONB
```

**Description**:
Updates multiple array elements with different values in one operation.

**Parameters**:
- `data` (JSONB): The JSONB data to update
- `array_path` (TEXT[]): Path to the array
- `match_key` (TEXT): Key to match elements on
- `updates` (JSONB): Array of update objects with `id` and new values

**Returns**:
- `JSONB`: Updated data

**Performance**: 3-5× faster than sequential updates

**Example**:
```sql
-- Update multiple order items
UPDATE tv_orders SET data = jsonb_array_update_where_batch(
    data,
    ARRAY['items'],
    'id',
    '[
        {"id": "item-1", "price": 15.99},
        {"id": "item-2", "price": 25.99}
    ]'::jsonb
)
WHERE pk_order = 1;
```

---

### Fallback Path Operations

#### jsonb_delta_set_path()

Flexible path-based JSONB updates with fallback support.

**Signature**:
```sql
jsonb_delta_set_path(
    data JSONB,
    path TEXT,
    value JSONB
) RETURNS JSONB
```

**Description**:
Updates JSONB data at a specified path, with graceful fallback when jsonb_ivm is unavailable.

**Parameters**:
- `data` (JSONB): The JSONB data to update
- `path` (TEXT): Dot-notation path (e.g., `'customer.name'`)
- `value` (JSONB): New value

**Returns**:
- `JSONB`: Updated data

**Performance**: 2× faster than jsonb_set when jsonb_delta available

**Example**:
```sql
-- Update order status
UPDATE tv_orders SET data = jsonb_delta_set_path(
    data,
    'status',
    '"shipped"'::jsonb
)
WHERE pk_order = 1;
```

---

### Security & Validation

All jsonb_ivm integration functions include comprehensive security validation:

- **SQL Injection Prevention**: All identifiers are validated using `validate_sql_identifier()`
- **Path Validation**: JSONB paths are validated using `validate_jsonb_path()`
- **Graceful Degradation**: Functions work with or without jsonb_ivm extension
- **Error Handling**: Clear error messages for invalid inputs

### Performance Characteristics

| Function | Performance Gain | Use Case |
|----------|------------------|----------|
| `extract_jsonb_id` | 5× faster | ID field extraction |
| `check_array_element_exists` | 10× faster | Array element existence checks |
| `jsonb_delta_array_update_where_path` | 2-3× faster | Nested field updates in arrays |
| `jsonb_array_update_where_batch` | 3-5× faster | Bulk array element updates |
| `jsonb_delta_set_path` | 2× faster | Flexible path-based updates |

### Fallback Behavior

When `jsonb_delta` extension is not available, all functions automatically fall back to standard PostgreSQL JSONB operations:

- Performance warnings are logged
- Results remain identical
- No functionality is lost
- Applications continue to work seamlessly

## Common Usage Patterns

### Check Extension Status
```sql
-- Verify extension is installed
SELECT pg_tviews_version();

-- Check for optional performance extension
SELECT pg_tviews_check_jsonb_ivm();
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
-- Step 1: Begin transaction with changes
BEGIN;
INSERT INTO tb_post (fk_user, title) VALUES (1, 'New Post');

-- Step 2: Prepare transaction (queue is persisted)
PREPARE TRANSACTION 'txn-123';

-- Step 3a: Commit (in another session)
COMMIT PREPARED 'txn-123';
SELECT pg_tviews_commit_prepared('txn-123');

-- OR Step 3b: Rollback
ROLLBACK PREPARED 'txn-123';
SELECT pg_tviews_rollback_prepared('txn-123');
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
- 2PC functions require careful coordination in distributed systems

### Common Pitfalls
- Don't use manual operations in triggers (causes recursion)
- 2PC GIDs must be unique per prepared transaction
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
ERROR:  permission denied for function pg_tviews_commit_prepared
```
**Solution**: 2PC functions require superuser or specific GRANT permissions.

### Invalid TVIEW Name
```sql
ERROR: TVIEW name must follow tv_* convention
```
**Solution**: Use names like `tv_user`, `tv_post`, etc.

## See Also

- [Monitoring Guide](../operations/monitoring.md)
- [Troubleshooting Guide](../operations/troubleshooting.md)
- [FraiseQL Integration Guide](../getting-started/fraiseql-integration.md)
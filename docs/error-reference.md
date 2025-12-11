# pg_tviews Error Reference

**Version**: 0.1.0-alpha
**Last Updated**: December 10, 2025

## Overview

This document provides comprehensive reference for all error types that can occur in pg_tviews. Each error includes its SQLSTATE code, common causes, and resolution steps.

## Error Categories

- [Metadata Errors](#metadata-errors) - TVIEW definition and catalog issues
- [Dependency Errors](#dependency-errors) - Relationship and cycle detection
- [SQL Parsing Errors](#sql-parsing-errors) - Query validation and syntax issues
- [Extension Dependency Errors](#extension-dependency-errors) - Required extension issues
- [Concurrency Errors](#concurrency-errors) - Locking and deadlock issues
- [Refresh Errors](#refresh-errors) - Runtime refresh operation failures
- [System Errors](#system-errors) - Internal and infrastructure issues

## Metadata Errors

### MetadataNotFound

**SQLSTATE**: P0001
**Description**: TVIEW metadata not found for the specified entity.

**Common Causes**:
- TVIEW was dropped but code still references it
- TVIEW creation failed partway through
- Database corruption or inconsistent state

**Example**:
```sql
-- Error occurs when trying to refresh a non-existent TVIEW
SELECT pg_tviews_cascade('unknown_entity'::regclass::oid, 123);
-- ERROR: TVIEW metadata not found for entity 'unknown_entity'
```

**Resolution**:
1. Verify TVIEW exists: `SELECT * FROM pg_tview_meta WHERE entity = 'entity_name';`
2. Recreate TVIEW if missing: `SELECT pg_tviews_create('entity_name', 'SELECT ...');`
3. Check for typos in entity name

**When to Report**: If TVIEW should exist but metadata is missing.

### TViewAlreadyExists

**SQLSTATE**: 42710
**Description**: Attempting to create a TVIEW that already exists.

**Common Causes**:
- Duplicate TVIEW creation attempts
- Migration scripts running multiple times
- Race conditions in application code

**Example**:
```sql
SELECT pg_tviews_create('tv_post', 'SELECT tb_post.pk_post, tb_post.id, jsonb_build_object(''id'', tb_post.id, ''data'', tb_post.data) as data FROM tb_post');
-- First call succeeds
SELECT pg_tviews_create('tv_post', 'SELECT tb_post.pk_post, tb_post.id, jsonb_build_object(''id'', tb_post.id, ''data'', tb_post.data) as data FROM tb_post');
-- ERROR: TVIEW 'posts' already exists
```

**Resolution**:
1. Check if TVIEW exists: `SELECT * FROM pg_tview_meta WHERE entity = 'posts';`
2. Use conditional creation: `SELECT pg_tviews_drop('posts', true);` then recreate
3. Implement idempotent creation logic in application code

### InvalidTViewName

**SQLSTATE**: 42602
**Description**: TVIEW name doesn't follow required naming convention.

**Common Causes**:
- TVIEW name doesn't start with `tv_`
- Invalid characters in name
- Name conflicts with PostgreSQL reserved words

**Example**:
```sql
SELECT pg_tviews_create('posts', 'SELECT ...');  -- Missing tv_ prefix
-- ERROR: Invalid TVIEW name 'posts': must start with 'tv_'
```

**Resolution**:
1. Use correct naming: `tv_<entity>` format
2. Check for reserved words: `SELECT word FROM pg_get_keywords() WHERE word = 'name';`
3. Use underscores for multi-word entities: `tv_user_posts`

## Dependency Errors

### CircularDependency

**SQLSTATE**: 55P03
**Description**: Circular dependency detected between TVIEWs.

**Common Causes**:
- TVIEW A depends on TVIEW B which depends on TVIEW A
- Complex dependency chains creating loops
- Incorrect foreign key relationships

**Example**:
```sql
-- tv_post depends on users, tv_user depends on posts
CREATE TABLE tv_post AS SELECT tb_post.pk_post, tb_post.id, jsonb_build_object('id', tb_post.id, 'userName', tb_user.name) as data FROM tb_post JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;
CREATE TABLE tv_user AS SELECT tb_user.pk_user, tb_user.id, jsonb_build_object('id', tb_user.id, 'postTitle', tb_post.title) as data FROM tb_user JOIN tb_post ON tb_user.pk_user = tb_post.fk_user;
-- ERROR: Circular dependency detected: posts → users → posts
```

**Resolution**:
1. Redesign schema to eliminate circular dependencies
2. Use computed columns instead of joins where possible
3. Restructure data model to use one-way relationships

**When to Report**: If circular dependency is legitimate and should be supported.

### DependencyDepthExceeded

**SQLSTATE**: 54001
**Description**: Dependency chain exceeds maximum allowed depth.

**Common Causes**:
- Deep dependency hierarchies
- Recursive relationships
- Overly complex data models

**Example**:
```sql
-- Chain: tv_a -> tv_b -> tv_c -> ... -> tv_z (26 levels deep)
-- ERROR: Dependency depth 26 exceeds maximum 10
```

**Resolution**:
1. Simplify dependency chain by denormalizing intermediate levels
2. Use direct table access instead of cascading through multiple TVIEWs
3. Increase `max_propagation_depth` if absolutely necessary (not recommended)

### DependencyResolutionFailed

**SQLSTATE**: 55000
**Description**: Failed to resolve dependencies for a TVIEW.

**Common Causes**:
- Referenced tables don't exist
- Foreign key relationships not properly defined
- Schema changes after TVIEW creation

**Example**:
```sql
CREATE TABLE tv_post AS SELECT tb_post.pk_post, tb_post.id, jsonb_build_object('id', tb_post.id, 'userName', tb_user.name) as data FROM tb_post JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;
-- Later: DROP TABLE users;
-- Then refresh: ERROR: Failed to resolve dependencies for 'tv_post': table 'users' does not exist
```

**Resolution**:
1. Verify all referenced tables exist
2. Check foreign key constraints are intact
3. Recreate TVIEW after schema changes: `DROP TABLE tv_post; CREATE TABLE tv_post AS ...;`

## SQL Parsing Errors

### InvalidSelectStatement

**SQLSTATE**: 42601
**Description**: The SELECT statement provided is invalid or unsupported.

**Common Causes**:
- Syntax errors in SQL
- Unsupported SQL features (UNION, CTEs, window functions)
- Missing required columns (pk_*, data)

**Example**:
```sql
SELECT pg_tviews_create('posts', 'SELECT id FROM posts');  -- Missing pk_post and data columns
-- ERROR: Invalid SELECT statement: Required column 'pk_post' missing
```

**Resolution**:
1. Ensure SELECT includes required columns:
   - `pk_<entity>` as primary key column
   - `data` as JSONB column
2. Check SQL syntax with `EXPLAIN SELECT ...;`
3. Remove unsupported features (see DDL Reference for supported SQL)

### RequiredColumnMissing

**SQLSTATE**: 42703
**Description**: Required column is missing from SELECT statement.

**Common Causes**:
- Missing `pk_<entity>` column
- Missing `data` column
- Incorrect column naming

**Example**:
```sql
SELECT pg_tviews_create('tv_post', 'SELECT tb_post.id, tb_post.title FROM tb_post');
-- ERROR: Required column 'pk_post' missing in SELECT statement
```

**Resolution**:
1. Add primary key column: `SELECT id as pk_post, ...`
2. Add data column: `SELECT ..., jsonb_build_object(...) as data`
3. Verify column names match entity: `pk_<entity>`

### TypeInferenceFailed

**SQLSTATE**: 42804
**Description**: Failed to infer PostgreSQL column types.

**Common Causes**:
- Complex expressions that can't be typed
- User-defined types not recognized
- Table doesn't exist or has no rows

**Example**:
```sql
SELECT pg_tviews_create('posts', 'SELECT complex_function(id) as pk_post, data FROM posts');
-- ERROR: Failed to infer type for column 'pk_post': complex_function not recognized
```

**Resolution**:
1. Use explicit type casts: `SELECT id::bigint as pk_post`
2. Simplify expressions or use subqueries
3. Ensure referenced tables exist and have data

## Extension Dependency Errors

### JsonbIvmNotInstalled

**SQLSTATE**: 58P01
**Description**: Required jsonb_ivm extension is not installed.

**Common Causes**:
- jsonb_ivm extension not installed
- Performance optimization disabled
- Fresh PostgreSQL installation

**Example**:
```sql
-- During TVIEW creation or refresh
-- WARNING: jsonb_ivm extension not detected
-- → Performance: Basic (full document replacement)
-- → To enable 1.5-3× faster cascades, install jsonb_ivm
```

**Resolution**:
1. Install jsonb_ivm: `CREATE EXTENSION jsonb_ivm;`
2. Download from: https://github.com/fraiseql/jsonb_ivm
3. Restart pg_tviews extension if needed

**Note**: This is a warning, not an error. pg_tviews works without jsonb_ivm but slower.

### ExtensionVersionMismatch

**SQLSTATE**: 58P01
**Description**: Extension version incompatibility detected.

**Common Causes**:
- Outdated extension version
- Incompatible PostgreSQL version
- Mixed extension versions in cluster

**Example**:
```sql
-- During extension load
-- ERROR: Extension 'pg_tviews' version mismatch: required 0.1.0, found 0.0.9
```

**Resolution**:
1. Update extension: `ALTER EXTENSION pg_tviews UPDATE;`
2. Reinstall if needed: `DROP EXTENSION pg_tviews; CREATE EXTENSION pg_tviews;`
3. Check PostgreSQL compatibility

## Concurrency Errors

### LockTimeout

**SQLSTATE**: 40P01
**Description**: Lock acquisition timed out.

**Common Causes**:
- Long-running transactions blocking TVIEW operations
- High concurrency causing lock contention
- Deadlock prevention mechanisms

**Example**:
```sql
-- During high concurrency
-- ERROR: Lock timeout on resource 'pg_tview_meta' after 30000ms
```

**Resolution**:
1. Reduce transaction length
2. Use shorter lock timeouts: `SET lock_timeout = '10s';`
3. Implement retry logic with exponential backoff
4. Check for long-running transactions: `SELECT * FROM pg_stat_activity WHERE state = 'active';`

### DeadlockDetected

**SQLSTATE**: 40P01
**Description**: Deadlock detected between concurrent operations.

**Common Causes**:
- Multiple TVIEWs modifying each other
- Complex dependency chains with concurrent updates
- Poor transaction ordering

**Example**:
```sql
-- Two transactions updating interdependent TVIEWs
-- Transaction 1: UPDATE posts SET ... WHERE id = 1;
-- Transaction 2: UPDATE users SET ... WHERE id = 1;
-- ERROR: Deadlock detected in TVIEW refresh operation
```

**Resolution**:
1. Reorder operations to avoid deadlocks
2. Use shorter transactions
3. Implement deadlock retry logic
4. Review dependency graph for cycles

## Refresh Errors

### CascadeDepthExceeded

**SQLSTATE**: 54001
**Description**: Cascade refresh exceeded maximum depth limit.

**Common Causes**:
- Deep dependency chains
- Infinite loops in refresh logic
- Recursive relationships

**Example**:
```sql
-- Deep dependency chain causes cascade to exceed limit
-- ERROR: Cascade depth 15 exceeds maximum 10. Possible infinite cascade loop.
```

**Resolution**:
1. Simplify dependency chain
2. Increase `max_propagation_depth` if needed (not recommended)
3. Check for infinite loops in business logic
4. Use direct table access instead of cascading

### RefreshFailed

**SQLSTATE**: XX000
**Description**: Individual TVIEW refresh operation failed.

**Common Causes**:
- Data corruption in base tables
- Constraint violations during refresh
- Permission issues on TVIEW tables

**Example**:
```sql
-- During automatic refresh
-- ERROR: Failed to refresh TVIEW 'post' row 123: permission denied for table tv_post
```

**Resolution**:
1. Check permissions on TVIEW tables
2. Verify data integrity in base tables
3. Check for constraint violations
4. Review TVIEW definition for errors

### BatchTooLarge

**SQLSTATE**: 54000
**Description**: Batch operation exceeds size limits.

**Common Causes**:
- Large bulk operations
- Configuration limits too low
- Memory constraints

**Example**:
```sql
-- Large bulk insert
-- ERROR: Batch size 5000 exceeds maximum 1000
```

**Resolution**:
1. Process in smaller batches
2. Increase batch size limits if hardware allows
3. Use statement-level triggers for bulk operations

### DependencyCycle

**SQLSTATE**: 55P03
**Description**: Dependency cycle detected in entity graph.

**Common Causes**:
- Circular relationships in data model
- Incorrect foreign key setup
- Complex many-to-many relationships

**Example**:
```sql
-- ERROR: Dependency cycle detected in entity graph: posts -> comments -> posts
```

**Resolution**:
1. Redesign schema to eliminate cycles
2. Use computed columns or triggers instead of circular TVIEWs
3. Implement cycle detection in application logic

### PropagationDepthExceeded

**SQLSTATE**: 54001
**Description**: Propagation exceeded maximum depth (possible infinite loop).

**Common Causes**:
- Deep dependency chains
- Recursive business logic
- Infinite loops in refresh propagation

**Example**:
```sql
-- ERROR: Propagation exceeded maximum depth of 50 iterations (100 entities processed)
```

**Resolution**:
1. Simplify dependency graph
2. Check for infinite loops in business rules
3. Increase depth limit if necessary (not recommended)
4. Use direct updates instead of cascading

## System Errors

### CatalogError

**SQLSTATE**: XX000
**Description**: PostgreSQL catalog operation failed.

**Common Causes**:
- Database corruption
- Permission issues on system catalogs
- PostgreSQL internal errors

**Example**:
```sql
-- ERROR: Catalog operation 'get_table_oid' failed: permission denied for table pg_class
```

**Resolution**:
1. Check database permissions
2. Run `ANALYZE;` to update statistics
3. Check PostgreSQL logs for catalog corruption
4. Consider `REINDEX SYSTEM;` for catalog issues

**When to Report**: Usually indicates PostgreSQL issues, not pg_tviews bugs.

### SpiError

**SQLSTATE**: XX000
**Description**: Server Programming Interface (SPI) operation failed.

**Common Causes**:
- SQL syntax errors in generated queries
- Permission issues
- Resource constraints

**Example**:
```sql
-- ERROR: SPI query failed: permission denied for table tv_post
-- Query: UPDATE tv_post SET data = $1 WHERE pk_post = $2
```

**Resolution**:
1. Check permissions on TVIEW tables
2. Verify generated SQL syntax
3. Check resource limits (work_mem, etc.)
4. Review PostgreSQL error logs

### SerializationError

**SQLSTATE**: XX000
**Description**: Data serialization/deserialization failed.

**Common Causes**:
- JSONB data corruption
- Binary data format changes
- Memory allocation failures

**Example**:
```sql
-- ERROR: Serialization error: JSON parse error: invalid character
```

**Resolution**:
1. Check data integrity in base tables
2. Validate JSONB data: `SELECT jsonb_typeof(data) FROM tv_table;`
3. Repair corrupted data
4. Check available memory

### ConfigError

**SQLSTATE**: XX000
**Description**: Configuration setting error.

**Common Causes**:
- Invalid GUC values
- Configuration conflicts
- Environment variable issues

**Example**:
```sql
-- ERROR: Configuration error for 'pg_tviews.max_propagation_depth': invalid value 'abc' (value: abc)
```

**Resolution**:
1. Check configuration syntax: `SHOW pg_tviews.max_propagation_depth;`
2. Use valid values: `SET pg_tviews.max_propagation_depth = 10;`
3. Check postgresql.conf for invalid settings

### CacheError

**SQLSTATE**: XX000
**Description**: Cache operation failed.

**Common Causes**:
- Memory corruption
- Concurrent access issues
- Cache size limits exceeded

**Example**:
```sql
-- ERROR: Cache 'graph_cache' error: poisoned mutex
```

**Resolution**:
1. Restart PostgreSQL to clear corrupted cache
2. Check memory usage and limits
3. Reduce cache sizes if needed
4. Check for concurrent access issues

### CallbackError

**SQLSTATE**: XX000
**Description**: FFI callback operation failed.

**Common Causes**:
- Panics in Rust code
- Memory corruption in FFI boundary
- Threading issues

**Example**:
```sql
-- ERROR: FFI callback 'trigger_handler' failed: panicked at 'index out of bounds'
```

**Resolution**:
1. Check PostgreSQL logs for panic details
2. Restart PostgreSQL
3. Report as bug with full error context

**When to Report**: Always report callback errors as they indicate Rust panics.

### MetricsError

**SQLSTATE**: XX000
**Description**: Metrics collection operation failed.

**Common Causes**:
- Metrics table corruption
- Permission issues on metrics
- Disk space issues

**Example**:
```sql
-- ERROR: Metrics operation 'record_timing' failed: disk full
```

**Resolution**:
1. Check disk space: `df -h`
2. Clean old metrics: `SELECT pg_tviews_cleanup_metrics(30);`
3. Check metrics table permissions
4. Repair corrupted metrics table if needed

### InternalError

**SQLSTATE**: XX000
**Description**: Internal error indicating a bug in pg_tviews.

**Common Causes**:
- Programming errors in extension
- Unexpected state transitions
- Logic errors in refresh algorithms

**Example**:
```sql
-- ERROR: Internal error at src/refresh/main.rs:123: assertion failed: queue.is_empty()
-- Please report this bug.
```

**Resolution**:
1. Note the file and line number
2. Gather context: TVIEW definition, data sample, operation performed
3. Report as bug with full error details

**When to Report**: Always report internal errors as they indicate bugs in the extension.

## Error Handling Best Practices

### Application-Level Error Handling

```sql
-- Use try-catch style error handling
CREATE OR REPLACE FUNCTION safe_tview_operation()
RETURNS void
LANGUAGE plpgsql
AS $$
BEGIN
    -- Attempt operation
    PERFORM pg_tviews_create('tv_post', 'SELECT ...');
EXCEPTION
    WHEN sqlstate '42710' THEN  -- Duplicate object
        RAISE NOTICE 'TVIEW already exists, skipping';
    WHEN sqlstate 'P0001' THEN  -- TVIEW metadata not found
        RAISE NOTICE 'TVIEW missing, recreating';
        PERFORM pg_tviews_create('tv_post', 'SELECT ...');
    WHEN OTHERS THEN
        RAISE;  -- Re-raise unexpected errors
END;
$$;
```

### Logging and Monitoring

```sql
-- Log errors for monitoring
CREATE OR REPLACE FUNCTION log_tview_errors()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'ERROR' THEN
        INSERT INTO error_log (error_time, error_message, context)
        VALUES (now(), TG_ERROR_MESSAGE, TG_ARGV[0]);
    END IF;
    RETURN NULL;
END;
$$;
```

### Error Recovery Strategies

1. **Idempotent Operations**: Design operations that can be safely retried
2. **Graceful Degradation**: Fall back to direct table access when TVIEWs fail
3. **Circuit Breakers**: Temporarily disable TVIEWs during extended outages
4. **Automated Recovery**: Implement health checks and automatic recreation

## See Also

- [Debugging Guide](operations/debugging.md) - Troubleshooting procedures
- [API Reference](API_REFERENCE.md) - Function documentation
- [Monitoring Guide](MONITORING.md) - Health checking and metrics
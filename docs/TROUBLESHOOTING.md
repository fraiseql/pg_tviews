# pg_tviews Troubleshooting Guide

This guide covers common issues and their solutions.

## Benchmark-Related Issues

### 1. "syntax error at or near :"

**Symptom**:
```
psql:data/01_ecommerce_data.sql:151: ERROR: syntax error at or near ":"
LINE 3:     v_scale TEXT := :'data_scale';  -- Use psql variable: sm...
                            ^
```

**Cause**: Incorrect psql variable interpolation syntax in DO blocks

**Solution**: Use temp table to pass psql variables into PL/pgSQL

**Wrong**:
```sql
DO $$
DECLARE
    v_scale TEXT := :'data_scale';  -- Doesn't work in DO blocks
BEGIN
    -- code
END $$;
```

**Correct**:
```sql
-- Create temp table with scale
CREATE TEMP TABLE temp_scale (scale_value TEXT);
INSERT INTO temp_scale VALUES (:'data_scale');

DO $$
DECLARE
    v_scale TEXT;
BEGIN
    SELECT scale_value INTO v_scale FROM temp_scale LIMIT 1;
    -- code using v_scale
END $$;
```

**Why This Happens**: Psql variable interpolation doesn't work inside DO blocks (string literals to PostgreSQL).

### 2. `CREATE TABLE tv_x AS SELECT …` does not create a TVIEW

**Symptom**: the statement creates a plain table, or is refused, instead of a TVIEW.

**Cause**: `CREATE TABLE tv_<entity> AS SELECT …` is turned into a TVIEW by a hook
installed when the library is loaded. Without `pg_tviews` in
`shared_preload_libraries`, a session that has not loaded the library yet does not
see it.

**Solution**: add `pg_tviews` to `shared_preload_libraries` and restart, or create
the TVIEW with the function, which works in any session:

```sql
SELECT tviews.pg_tviews_create_or_replace('tv_test', 'SELECT pk_test, id, data FROM tb_test');
```

### 3. "relation does not exist"

**Symptom**:
```
ERROR: relation "tv_product" does not exist
```

**Cause**: Missing schema qualification or incorrect search_path

**Solution 1: Use Schema-Qualified Names**
```sql
-- Wrong
SELECT * FROM tv_product;

-- Correct
SELECT * FROM benchmark.tv_product;
```

**Solution 2: Set Search Path**
```sql
SET search_path TO benchmark, public;
SELECT * FROM tv_product;  -- Now works
```

**Diagnostic**:
```bash
# Check which schema the table is in
psql -d pg_tviews_benchmark -c "
SELECT schemaname, tablename
FROM pg_tables
WHERE tablename = 'tv_product';
"
```

### 4. Variable Quoting Issues in Shell Scripts

**Symptom**: Scenarios fail with variable interpolation errors

**Cause**: Inconsistent quoting between data generation and scenarios

**Wrong**:
```bash
# Double-quoting issue
$PSQL -v data_scale="'$scale'" -f scenarios/file.sql
# Results in: data_scale='small' (quotes part of value)
```

**Correct**:
```bash
# Single variable assignment
$PSQL -v data_scale="$scale" -f scenarios/file.sql
# Results in: data_scale=small (clean value)
```

**Rule**: Let psql handle quoting in SQL, not in shell

## TVIEW-Related Issues

### 5. "Table validation failed: missing required columns"

**Symptom**:
```
ERROR: Table validation failed: missing required columns: id, data
```

**Cause**: Table missing required columns for TVIEW

**Solution**: Ensure table has minimum required columns

**Minimum TVIEW Structure**:
```sql
CREATE TABLE tv_entity AS
SELECT
    id,    -- UUID (required)
    data   -- JSONB (required)
FROM v_entity;
```

**Recommended TVIEW Structure** (with optimizations):
```sql
CREATE TABLE tv_entity AS
SELECT
    id,           -- UUID (required)
    pk_entity,    -- INTEGER primary key (recommended)
    fk_parent,    -- INTEGER foreign key (for filtering)
    parent_id,    -- UUID foreign key (for joins)
    path,         -- LTREE (for hierarchical queries)
    data          -- JSONB (required)
FROM v_entity;
```

**Verification**:
```bash
# Check table structure
psql -d pg_tviews_benchmark -c "\d benchmark.tv_product"
```

### 6. A pg_tviews function does not exist

**Symptom**:
```
ERROR: function pg_tviews_create_or_replace(unknown, unknown) does not exist
```

**Cause**: the extension is not created in this database, or its schema `tviews` is
not on the `search_path`.

**Solution**: qualify the call (`tviews.pg_tviews_create_or_replace(…)`), add
`tviews` to the database's `search_path`, or create the extension
```sql
CREATE EXTENSION IF NOT EXISTS pg_tviews;
```

**Verification**:
```bash
# Check extension is loaded
psql -d pg_tviews_benchmark -c "\dx pg_tviews"

# List TVIEW functions
psql -d pg_tviews_benchmark -c "\df tviews.pg_tviews*"
```

## Diagnostic Commands

### Check Schema State
```bash
psql -d pg_tviews_benchmark <<EOF
SELECT schemaname, tablename
FROM pg_tables
WHERE schemaname IN ('benchmark', 'public')
ORDER BY schemaname, tablename;
EOF
```

### Check Data Loading
```bash
psql -d pg_tviews_benchmark <<EOF
SELECT
    'tb_category' as table,
    COUNT(*) as row_count
FROM benchmark.tb_category
UNION ALL
SELECT 'tb_product', COUNT(*)
FROM benchmark.tb_product
ORDER BY table;
EOF
```

### Check TVIEW Status
```bash
psql -d pg_tviews_benchmark <<EOF
SELECT schema, name, view, base_tables, needs_reregister
FROM tviews.registry
ORDER BY schema, name;

SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check();
EOF
```

### Re-create a TVIEW from its definition
```bash
psql -d pg_tviews_benchmark <<EOF
SELECT tviews.pg_tviews_create_or_replace('benchmark.tv_product',
    (SELECT query FROM tviews.registry WHERE schema = 'benchmark' AND name = 'tv_product'));
EOF
```

### Check Docker Container Status
```bash
# Container status
docker compose ps

# Recent logs
docker compose logs --tail=50

# Database logs specifically
docker compose logs postgres | tail -50
```

### Full Benchmark Diagnostic
```bash
# Run benchmark with full logging (the old comprehensive_benchmarks harness was
# removed — it never ran against the real extension; use test/sql/real_benchmark/)
cd test/sql/real_benchmark
PGHOST=localhost PGPORT=28818 PGUSER=postgres ./run.sh --scales small 2>&1 | tee /tmp/benchmark_debug.log

# Check for errors
grep -i "error" /tmp/benchmark_debug.log | grep -v "0 errors"

# Check for successes
grep -iE "success|complete" /tmp/benchmark_debug.log
```

## Getting Help

If you're stuck after trying the solutions above:

1. **Capture diagnostics**:
   ```bash
   # Run all diagnostic commands above
   # Save output to a file
   ```

2. **Note exact error messages**:
   - Copy the full error (not paraphrased)
   - Include line numbers if shown
   - Include relevant code context

3. **Check git history**:
   ```bash
   git log --oneline -10
   # Recent changes may have introduced issues
   ```

4. **Ask for help with context**:
   - What you were trying to do
   - What command you ran
   - Full error message
   - What you've tried already
   - Diagnostic output

5. **Search issues**:
   - Check project issues for similar problems
   - Search error message text

## Performance Issues

### Benchmark Runs Slowly

**Symptom**: Benchmark takes >30 minutes for small scale

**Possible Causes**:
1. Cold Docker cache (first run)
2. Insufficient resources (RAM/CPU)
3. Disk I/O bottleneck

**Solutions**:
```bash
# Check Docker resources
docker stats

# Check disk I/O
iostat -x 5

# Increase Docker resources
# Edit Docker Desktop settings: Memory > 4GB, CPUs > 2
```

### Query Performance Regression

**Symptom**: Queries slower than expected

**Diagnostic**:
```sql
EXPLAIN ANALYZE SELECT * FROM benchmark.tv_product WHERE ...;
```

**Common Issues**:
- Missing indexes on optimization columns
- TVIEW not converted (querying raw table)
- TVIEW content differs from its view (rebuild with `SELECT tviews.pg_tviews_refresh('product');`)

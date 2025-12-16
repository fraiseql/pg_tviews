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

### 2. "SPI error: Transaction"

**Symptom**:
```
ERROR: Failed to convert table to TVIEW: SPI query failed: SPI error: Transaction
Query: Unknown
```

**Cause**: Event triggers cannot use SPI during DDL events (PostgreSQL limitation)

**Solution**: Use manual conversion workflow

**Steps**:
```sql
-- 1. Create table (event trigger validates structure only)
CREATE TABLE tv_test AS SELECT id, data FROM v_test;

-- 2. Manually convert to TVIEW
SELECT pg_tviews_convert_existing_table('tv_test');

-- 3. Verify
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_test';
```

**Why This Happens**: PostgreSQL prevents nested transactions during DDL events. SPI calls create sub-transactions, causing conflicts.

**Future**: Background worker support will enable automatic conversion in a separate transaction context.

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

### 6. Manual Conversion Function Doesn't Exist

**Symptom**:
```
ERROR: function pg_tviews_convert_existing_table(text) does not exist
```

**Cause**: Extension not loaded in current database

**Solution**: Load the extension
```sql
CREATE EXTENSION IF NOT EXISTS pg_tviews;
```

**Verification**:
```bash
# Check extension is loaded
psql -d pg_tviews_benchmark -c "\dx pg_tviews"

# List TVIEW functions
psql -d pg_tviews_benchmark -c "\df pg_tviews*"
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
SELECT
    table_name,
    source_view,
    created_at,
    last_refreshed
FROM pg_tviews_metadata
ORDER BY table_name;
EOF
```

### Test Manual Conversion
```bash
psql -d pg_tviews_benchmark <<EOF
-- Attempt conversion
SELECT pg_tviews_convert_existing_table('benchmark.tv_product');

-- Check result
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_product';
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
# Run benchmark with full logging
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale small 2>&1 | tee /tmp/benchmark_debug.log

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
- Outdated TVIEW data (needs refresh)

---

*Last Updated: 2025-12-14*
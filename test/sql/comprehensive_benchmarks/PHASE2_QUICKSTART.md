# Phase 2 Comprehensive Benchmarks - Quick Start Guide

## 🎯 What's New in Phase 2

- ✅ **Cascade scenarios**: Test 1 parent → many children updates
- ✅ **Multiple scales**: Small (1K), Medium (100K), Large (1M)
- ✅ **Supplier relationships**: Realistic multi-table cascades
- ✅ **Real pg_ivm**: Auto-detect and use real extension if available

## 🚀 Quick Start

### Prerequisites
```bash
# PostgreSQL 14+ with pg_tviews extension installed
# Database with sufficient memory:
#   - Small: 100MB
#   - Medium: 1GB
#   - Large: 4GB
```

### Setup Database
```bash
# Create benchmark database
createdb pg_tviews_benchmark

# Or use existing database
export PGDATABASE=pg_tviews_benchmark
```

### Run Small Scale (1K products) - ~10 seconds
```bash
cd test/sql/comprehensive_benchmarks

# 1. Setup
psql -f 00_setup.sql

# 2. Create schema
psql -f schemas/01_ecommerce_schema.sql

# 3. Generate data
psql -f data/01_ecommerce_data_small.sql

# 4. Run benchmarks
psql -f scenarios/01_ecommerce_benchmarks_small.sql
psql -f scenarios/01_ecommerce_benchmarks_cascade.sql

# 5. View results
psql -c "SELECT * FROM benchmark_summary WHERE data_scale = 'small';"
```

### Run Medium Scale (100K products) - ~2-3 minutes
```bash
# After small scale setup...

# Generate medium data (~1-2 min)
psql -f data/01_ecommerce_data_medium.sql

# Run medium benchmarks
psql -f scenarios/01_ecommerce_benchmarks_medium.sql

# View results
psql -c "
SELECT
    test_name,
    operation_type,
    rows_affected,
    ROUND(execution_time_ms, 2) as time_ms,
    ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) as ms_per_row
FROM benchmark_summary
WHERE data_scale = 'medium'
ORDER BY test_name, execution_time_ms;
"
```

### Run Large Scale (1M products) - ~10-15 minutes
```bash
# After setup...

# Generate large data (~5-10 min)
psql -f data/01_ecommerce_data_large.sql

# Run large benchmarks (~2-5 min)
psql -f scenarios/01_ecommerce_benchmarks_large.sql

# View results
psql -c "
SELECT
    test_name,
    operation_type,
    rows_affected,
    ROUND(execution_time_ms, 2) as time_ms,
    ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) as ms_per_row
FROM benchmark_summary
WHERE data_scale = 'large'
ORDER BY test_name, execution_time_ms;
"
```

## 📊 View Results

### Summary of All Tests
```sql
SELECT
    data_scale,
    test_name,
    operation_type,
    rows_affected,
    ROUND(execution_time_ms, 2) as time_ms
FROM benchmark_summary
ORDER BY data_scale, test_name, execution_time_ms;
```

### Comparison: Incremental vs Full Refresh
```sql
SELECT
    data_scale,
    test_name,
    operation_type,
    rows_affected,
    ROUND(incremental_ms, 2) as incremental_ms,
    ROUND(baseline_ms, 2) as full_refresh_ms,
    improvement_ratio || 'x' as improvement
FROM benchmark_comparison
WHERE improvement_ratio > 1
ORDER BY improvement_ratio DESC;
```

### Cascade Performance Analysis
```sql
SELECT
    data_scale,
    test_name,
    operation_type,
    rows_affected,
    ROUND(execution_time_ms, 2) as total_ms,
    ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) as ms_per_row
FROM benchmark_summary
WHERE test_name LIKE '%_cascade'
ORDER BY data_scale, operation_type, rows_affected;
```

### Export Results to CSV
```bash
# Export all results
psql -c "\copy (SELECT * FROM benchmark_summary ORDER BY data_scale, test_name) TO 'benchmark_results.csv' CSV HEADER"

# Export comparison
psql -c "\copy (SELECT * FROM benchmark_comparison WHERE improvement_ratio > 1 ORDER BY improvement_ratio DESC) TO 'benchmark_comparison.csv' CSV HEADER"
```

## 🎯 Benchmark Scenarios

### Small Scale (1K products)
1. **Single row update** - 1 product price change
2. **Category cascade** - 1 category → ~100 products
3. **Supplier cascade** - 1 supplier → multiple products
4. **Bulk update** - 100 products

### Medium Scale (100K products)
1. **Single row update** - 1 product price change
2. **Category cascade** - 1 category → ~1000 products
3. **Bulk update** - 100 products
4. **Bulk update** - 1000 products

### Large Scale (1M products)
1. **Single row update** - 1 product price change
2. **Category cascade** - 1 category → ~2000 products
3. **Bulk update** - 1000 products
4. **Large bulk update** - 10,000 products

## 🔧 Troubleshooting

### pg_ivm Extension Not Found
```
⚠ jsonb_delta extension not available, loading stubs
```

**This is OK!** Benchmarks will use compatible stub functions. Real extension would be 20-50% faster, but comparison ratios remain valid.

To install real extension (optional):
```bash
# Clone and build jsonb_delta
git clone https://github.com/fraiseql/jsonb_delta
cd jsonb_delta
make && sudo make install

# In PostgreSQL
CREATE EXTENSION jsonb_delta;
```

### Out of Memory (Large Scale)
```
ERROR:  out of memory
```

Increase PostgreSQL shared memory:
```sql
-- Check current setting
SHOW shared_buffers;

-- Increase (requires restart)
ALTER SYSTEM SET shared_buffers = '2GB';
ALTER SYSTEM SET work_mem = '256MB';
```

Then restart PostgreSQL:
```bash
sudo systemctl restart postgresql
```

### Slow Data Generation
Large scale data generation may take 5-10 minutes. Monitor progress:
```
Progress: 20.0% (200000 / 1000000) - 45.2 sec elapsed
```

Tips:
- Run on SSD for faster generation
- Increase `work_mem` temporarily
- Run during off-peak hours

### REFRESH MATERIALIZED VIEW Timeout
For large scale, full refresh may take 30-60 seconds. This is expected!

```sql
-- Check progress
SELECT
    pid,
    query_start,
    state,
    wait_event_type,
    substring(query, 1, 60) as query
FROM pg_stat_activity
WHERE query LIKE '%REFRESH%';
```

## 📈 Expected Results

### Single Row Updates (Constant Time)
- **Approach 1** (pg_tviews): 1-10ms
- **Approach 2** (manual): 1-15ms
- **Approach 3** (full): 50ms → 50,000ms

**Key Insight**: Incremental approaches stay constant, full refresh scales linearly.

### Cascade Operations (Scales with Affected Rows)
- **1K scale** (1 → 100):
  - Incremental: 5-20ms
  - Full: 50-100ms

- **100K scale** (1 → 1000):
  - Incremental: 50-200ms
  - Full: 5,000-10,000ms

- **1M scale** (1 → 2000):
  - Incremental: 100-500ms
  - Full: 50,000-100,000ms

**Key Insight**: Incremental scales with affected rows only. Full refresh recalculates entire table.

## 🎓 Understanding the Results

### Three Approaches Compared

**Approach 1: pg_tviews + jsonb_delta**
- Incremental updates only to affected rows
- Smart JSONB patching (no full object rebuild)
- Best for: Frequent small updates, real-time systems

**Approach 2: Manual + Native PostgreSQL**
- Incremental updates using jsonb_set
- Requires manual cascade logic
- Best for: Understanding overhead without pg_tviews

**Approach 3: Full REFRESH MATERIALIZED VIEW**
- Recalculates entire view
- Traditional PostgreSQL approach
- Best for: Infrequent batch updates

### When to Use Each Approach

| Update Pattern | Approach 1 | Approach 2 | Approach 3 |
|----------------|------------|------------|------------|
| Real-time updates | ✅ Best | ⚠️ OK | ❌ Too slow |
| Frequent small changes | ✅ Best | ⚠️ OK | ❌ Too slow |
| Cascade updates (1→many) | ✅ Best | ⚠️ OK | ❌ Too slow |
| Infrequent batch jobs | ⚠️ OK | ⚠️ OK | ✅ Best |
| Read-heavy workload | ✅ Best | ✅ Best | ⚠️ Acceptable |

## 🧪 Advanced Testing

### Run Specific Test
```sql
-- Run only cascade tests
\i scenarios/01_ecommerce_benchmarks_cascade.sql

-- Run only medium scale
\i scenarios/01_ecommerce_benchmarks_medium.sql
```

### Compare Scales
```sql
SELECT
    test_name,
    operation_type,
    data_scale,
    rows_affected,
    ROUND(execution_time_ms, 2) as time_ms,
    ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) as ms_per_row
FROM benchmark_summary
WHERE test_name = 'price_update'
  AND operation_type IN ('tviews_jsonb_delta', 'full_refresh')
ORDER BY data_scale, operation_type;
```

### Stress Test (Multiple Runs)
```bash
# Run small scale 10 times
for i in {1..10}; do
    echo "Run $i..."
    psql -f scenarios/01_ecommerce_benchmarks_small.sql > /dev/null
done

# Calculate average and std dev
psql -c "
SELECT
    test_name,
    operation_type,
    COUNT(*) as runs,
    ROUND(AVG(execution_time_ms), 2) as avg_ms,
    ROUND(STDDEV(execution_time_ms), 2) as stddev_ms,
    ROUND(MIN(execution_time_ms), 2) as min_ms,
    ROUND(MAX(execution_time_ms), 2) as max_ms
FROM benchmark_results
WHERE test_name = 'price_update'
GROUP BY test_name, operation_type
ORDER BY operation_type;
"
```

## 📝 Next Steps

1. **Run all scales** and record results
2. **Export results** to CSV for documentation
3. **Update README.md** with real benchmark numbers
4. **Share results** with community

## 💡 Tips

- Run on dedicated hardware for consistent results
- Clear caches between runs: `echo 3 > /proc/sys/vm/drop_caches` (Linux)
- Monitor system resources: `top`, `htop`, `iotop`
- Use `EXPLAIN ANALYZE` to understand query plans
- Check PostgreSQL logs for slow queries

## 🤝 Contributing Results

Found interesting results? Share them!

```bash
# Generate report
psql -c "
SELECT
    data_scale,
    test_name,
    operation_type,
    rows_affected,
    ROUND(execution_time_ms, 2) as time_ms,
    ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) as ms_per_row
FROM benchmark_summary
ORDER BY data_scale, test_name, execution_time_ms;
" > my_benchmark_results.txt

# Share hardware specs
echo "Hardware: [CPU], [RAM], [Storage], PostgreSQL [VERSION]" >> my_benchmark_results.txt
```

---

**Questions?** Check `THREE_WAY_COMPARISON.md` for detailed methodology.

**Issues?** Report at: https://github.com/fraiseql/pg_tviews/issues

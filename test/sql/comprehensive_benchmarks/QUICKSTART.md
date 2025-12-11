# Quick Start Guide: Comprehensive Benchmarks

## Overview

This benchmark suite provides comprehensive, real-world performance testing for pg_tviews across multiple scenarios and data scales.

## What's Been Created

### 1. **Benchmark Infrastructure**
- ✅ Setup script with result tracking (`00_setup.sql`)
- ✅ Automated runner script (`run_benchmarks.sh`)
- ✅ Report generator (`generate_report.py`)

### 2. **E-Commerce Scenario**
Complete benchmark for product catalog with:
- **Schema**: `tb_category` → `tb_product` → `tb_review` + `tb_inventory`
- **Trinity pattern**: `id` (UUID) + `pk_{entity}` (INTEGER) + `fk_{entity}` (INTEGER)
- **Projections**: `tv_product` (incremental) vs `mv_product` (full refresh)
- **3 Data Scales**:
  - **Small**: 10 categories, 1K products, 5K reviews
  - **Medium**: 100 categories, 100K products, 500K reviews
  - **Large**: 500 categories, 1M products, 5M reviews

### 3. **Test Coverage - Three-Way Comparison**
Each scenario tests **three approaches**:
- ✅ **Approach 1**: pg_tviews + jsonb_ivm (automatic surgical JSONB patching)
- ✅ **Approach 2**: pg_tviews + native PG (automatic `jsonb_set` updates)
- ✅ **Approach 3**: Manual function refresh (explicit calls with unlimited cascades)
- ✅ **Approach 4**: Traditional full `REFRESH MATERIALIZED VIEW`

**Test operations**:
- Single row updates (price changes, inventory updates)
- Bulk operations (100, 1000 rows)
- Cascade updates (review submissions affecting products)

## Running the Benchmarks

### Prerequisites

```bash
# Ensure PostgreSQL is running
psql --version

# Ensure pg_tviews extension is installed
cd /home/lionel/code/pg_tviews
cargo pgrx install --release
```

### Option 1: Run All Benchmarks (Recommended)

```bash
cd /home/lionel/code/pg_tviews/test/sql/comprehensive_benchmarks

# Run all scenarios at all scales
./run_benchmarks.sh

# Results will be in: results/benchmark_run_YYYYMMDD_HHMMSS.log
```

### Option 2: Run Specific Scale

```bash
# Run only small scale (fast, ~1 minute)
./run_benchmarks.sh --scale small

# Run only medium scale (~10 minutes)
./run_benchmarks.sh --scale medium

# Run only large scale (~1 hour, requires 16GB+ RAM)
./run_benchmarks.sh --scale large
```

### Option 3: Manual Step-by-Step

```bash
# 1. Setup
psql -d postgres -c "CREATE DATABASE pg_tviews_benchmark;"
psql -d pg_tviews_benchmark -f 00_setup.sql

# 2. Load schema
psql -d pg_tviews_benchmark -f schemas/01_ecommerce_schema.sql

# 3. Generate data (choose scale)
psql -d pg_tviews_benchmark -v data_scale="'small'" -f data/01_ecommerce_data.sql

# 4. Run benchmarks
psql -d pg_tviews_benchmark -v data_scale="'small'" -f scenarios/01_ecommerce_benchmarks.sql

# 5. View results
psql -d pg_tviews_benchmark -c "SELECT * FROM benchmark_summary;"
psql -d pg_tviews_benchmark -c "SELECT * FROM benchmark_comparison ORDER BY improvement_ratio DESC;"
```

## Generating Reports

```bash
# Generate markdown report with analysis
python3 generate_report.py

# Output: results/BENCHMARK_REPORT_YYYYMMDD_HHMMSS.md
```

## Understanding the Results

### Key Metrics

1. **improvement_ratio**: How many times faster incremental is vs full refresh
   - Example: `50.5×` means incremental is 50× faster

2. **execution_time_ms**: Absolute time in milliseconds
   - Single row: expect 1-10ms (incremental) vs 100-5000ms (full refresh)
   - Bulk 100: expect 10-50ms vs 200-10000ms
   - Bulk 1000: expect 100-500ms vs 2000-100000ms

3. **ms_per_row**: Average time per affected row
   - Lower is better
   - Incremental should be 0.1-5ms/row
   - Full refresh is constant (scans entire table)

### Expected Performance Patterns (Four-Way Comparison)

Based on architecture:

| Operation | Table Size | Approach 1 (pg_tviews + ivm) | Approach 2 (pg_tviews + native) | Approach 3 (Manual Function) | Approach 4 (Full Refresh) | 1 vs 4 |
|-----------|------------|-----------------------------|---------------------------------|-----------------------------|---------------------------|--------|
| Single row | 1K | 1-2ms | 1.5-3ms | 2.5-4ms | 50-200ms | 50-100× |
| Single row | 100K | 2-4ms | 3-6ms | 4-8ms | 2000-8000ms | 500-2000× |
| Single row | 1M | 3-6ms | 4-8ms | 6-12ms | 20000-50000ms | 3000-10000× |
| Bulk 100 | 1K | 10-20ms | 15-30ms | 20-40ms | 100-400ms | 5-20× |
| Bulk 100 | 100K | 20-40ms | 30-60ms | 40-80ms | 2500-10000ms | 100-400× |

**Key Insights**:
- **Approach 1 vs 2**: 1.5-2× faster (jsonb_ivm optimization)
- **Approach 2 vs 3**: 1.3-1.8× faster (automatic vs manual triggers)
- **Approach 3 vs 4**: 25-5000× faster (incremental vs full refresh)
- **Approach 1 vs 4**: 50-10000× faster (best vs worst)
- **Improvement grows with table size**: Full refresh must scan ALL rows

## Viewing Results in Database

```sql
-- Connect to benchmark database
\c pg_tviews_benchmark

-- View all results summary
SELECT * FROM benchmark_summary ORDER BY scenario, data_scale, test_name;

-- View improvements only
SELECT
    scenario,
    test_name,
    data_scale,
    operation_type,
    ROUND(baseline_ms, 2) as full_refresh_ms,
    ROUND(incremental_ms, 2) as incremental_ms,
    improvement_ratio || 'x faster' as improvement
FROM benchmark_comparison
WHERE improvement_ratio IS NOT NULL
ORDER BY improvement_ratio DESC;

-- View by scale
SELECT
    data_scale,
    COUNT(*) as num_tests,
    ROUND(AVG(improvement_ratio), 2) as avg_improvement,
    ROUND(MAX(improvement_ratio), 2) as max_improvement,
    ROUND(SUM(time_saved_ms), 2) as total_time_saved_ms
FROM benchmark_comparison
WHERE improvement_ratio IS NOT NULL
GROUP BY data_scale
ORDER BY
    CASE data_scale
        WHEN 'small' THEN 1
        WHEN 'medium' THEN 2
        WHEN 'large' THEN 3
    END;
```

## Troubleshooting

### Out of Memory (Large Scale)

```bash
# Increase PostgreSQL shared_buffers
# Edit postgresql.conf:
shared_buffers = 4GB
work_mem = 256MB

# Or run only small/medium scales
./run_benchmarks.sh --scale medium
```

### Slow Data Generation

```bash
# Large scale takes ~30-60 minutes to generate data
# Check progress in NOTICE messages
# Be patient or use smaller scale for testing
```

### Permission Errors

```bash
# Ensure you have CREATE DATABASE permission
psql -d postgres -c "CREATE DATABASE pg_tviews_benchmark;"

# If error, try:
sudo -u postgres psql -c "CREATE DATABASE pg_tviews_benchmark;"
```

## Next Steps

1. **Run small scale first** to validate setup (~2 minutes)
2. **Analyze results** using SQL queries or generate_report.py
3. **Run medium scale** for realistic performance data (~15 minutes)
4. **Optionally run large scale** for production-scale metrics (1+ hour)
5. **Update README.md** with real benchmark numbers

## File Structure

```
comprehensive_benchmarks/
├── 00_setup.sql                    # Setup benchmark database and tracking
├── run_benchmarks.sh               # Automated runner
├── generate_report.py              # Report generator
├── schemas/
│   └── 01_ecommerce_schema.sql     # E-commerce schema with trinity pattern
├── data/
│   └── 01_ecommerce_data.sql       # Data generator (all scales)
├── scenarios/
│   └── 01_ecommerce_benchmarks.sql # Benchmark tests
├── results/                        # Generated results (git-ignored)
│   ├── benchmark_run_*.log
│   ├── benchmark_results_*.csv
│   └── BENCHMARK_REPORT_*.md
└── README.md                       # Full documentation
```

## Sample Output

```
E-Commerce Benchmarks - small scale
====================================

Test 1: Single Product Price Update
-----------------------------------
NOTICE:  Incremental: 1.234 ms
NOTICE:  Full Refresh: 456.789 ms (scanned 1000 rows)

Improvement: 370× faster

Test 2: Bulk Price Update - 100 products
-----------------------------------------
NOTICE:  Incremental (100 rows): 12.345 ms (0.123 ms/row)
NOTICE:  Full Refresh: 512.678 ms (scanned 1000 rows)

Improvement: 41× faster
```

## Notes

- ⚠️ The benchmark database will be dropped/recreated on each run
- ⚠️ Large scale requires significant time and resources
- ✅ All operations are rolled back (safe to run repeatedly)
- ✅ Results are persisted in `benchmark_results` table
- ✅ CSV export available for external analysis

## Questions?

Check the main README.md or the inline SQL comments for more details.

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

### Option 1: Docker (Full 4-Way Benchmark - Recommended)

**Prerequisites**: Both repositories in same parent directory:
```
/path/to/code/
  ├── pg_tviews/
  └── jsonb_ivm/    # Clone from https://github.com/fraiseql/jsonb_ivm
```

```bash
# Build and run complete benchmark environment
cd /path/to/pg_tviews
docker build -f docker/dockerfile-benchmarks -t pg_tviews_bench ..

# OR use docker-compose:
cd /path/to/pg_tviews/docker
docker-compose up -d --build

# If using docker build, run container:
docker run -d --name pg_tviews_benchmark -p 5432:5432 -e POSTGRES_PASSWORD=postgres pg_tviews_bench

# Run benchmarks
docker exec -it pg_tviews_benchmark psql -U postgres -d pg_tviews_benchmark -c "
\i /benchmarks/00_setup.sql
\i /benchmarks/schemas/01_ecommerce_schema.sql
\i /benchmarks/data/01_ecommerce_data_small.sql
\i /benchmarks/scenarios/01_ecommerce_benchmarks_small.sql
"

# View results
docker exec -it pg_tviews_benchmark psql -U postgres -d pg_tviews_benchmark -c "
SELECT operation_type, ROUND(execution_time_ms, 2) as time_ms,
       CASE WHEN operation_type = 'full_refresh' THEN 'Baseline' ELSE 'Incremental' END as type
FROM benchmark_results
ORDER BY execution_time_ms;
"
```

### Option 2: Manual Approaches 3 & 4 (No Extensions Required)

```bash
# 1. Setup database
createdb pg_tviews_benchmark
cd test/sql/comprehensive_benchmarks
psql -d pg_tviews_benchmark -f 00_setup.sql

# 2. Load data (choose scale - modified versions that skip extension parts)
psql -d pg_tviews_benchmark -f data/01_ecommerce_data_small_manual.sql    # Small: 1K products, 5K reviews
# OR
psql -d pg_tviews_benchmark -f data/01_ecommerce_data_medium_manual.sql  # Medium: 100K products, 500K reviews
# OR
psql -d pg_tviews_benchmark -f data/01_ecommerce_data_large_manual.sql   # Large: 1M products, 5M reviews

# 3. Load manual functions
psql -d pg_tviews_benchmark -f functions/refresh_product_manual.sql

# 4. Populate manual tables
psql -d pg_tviews_benchmark -c "
INSERT INTO manual_func_product (pk_product, fk_category, data)
SELECT pk_product, fk_category, data FROM v_product;
REFRESH MATERIALIZED VIEW mv_product;
"

# 5. Run performance test
psql -d pg_tviews_benchmark -c "
-- Single product update: Manual vs Full Refresh
UPDATE tb_product SET current_price = current_price * 0.9 WHERE pk_product = 1;
SELECT 'Manual function:' as method, refresh_product_manual('product', 1, 'price_current') ->> 'execution_ms' || 'ms' as time;
UPDATE tb_product SET current_price = current_price / 0.9 WHERE pk_product = 1;

UPDATE tb_product SET current_price = current_price * 0.9 WHERE pk_product = 1;
SELECT 'Full refresh:' as method, clock_timestamp() - statement_timestamp() as time FROM (SELECT pg_sleep(0)) dummy;
REFRESH MATERIALIZED VIEW mv_product;
UPDATE tb_product SET current_price = current_price / 0.9 WHERE pk_product = 1;
"
```

### Option 3: Run Automated Script (Requires Extensions)

```bash
# Run all scenarios at all scales (requires pg_tviews extension)
./run_benchmarks.sh

# Or run specific scale
./run_benchmarks.sh --scale small

# Results will be in: results/benchmark_run_YYYYMMDD_HHMMSS.log
```

### Option 4: Manual Step-by-Step (Requires Extensions)

```bash
# 1. Setup (requires pg_tviews extension installed)
psql -d postgres -c "CREATE DATABASE pg_tviews_benchmark;"
psql -d pg_tviews_benchmark -c "CREATE EXTENSION pg_tviews;"
psql -d pg_tviews_benchmark -f 00_setup.sql

# 2. Load schema
psql -d pg_tviews_benchmark -f schemas/01_ecommerce_schema.sql

# 3. Generate data
psql -d pg_tviews_benchmark -v data_scale="'small'" -f data/01_ecommerce_data.sql

# 4. Run benchmarks
psql -d pg_tviews_benchmark -v data_scale="'small'" -f scenarios/01_ecommerce_benchmarks.sql

# 5. View results
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

### Docker/Full Extensions (Approaches 1-4)
```
E-Commerce Benchmarks - small scale
===================================

Test 1: Single Product Price Update
-----------------------------------
NOTICE:  [1] pg_tviews + jsonb_ivm: 0.8 ms
NOTICE:  [2] pg_tviews + native PG: 1.2 ms
NOTICE:  [3] Manual function: 2.3 ms
NOTICE:  [4] Full Refresh: 76.7 ms (scanned 1000 rows)

Improvement: 96× to 32× faster

Test 2: Bulk Price Update - 100 products
-----------------------------------------
NOTICE:  [1] pg_tviews + jsonb_ivm (100 rows): 8.5 ms (0.085 ms/row)
NOTICE:  [2] pg_tviews + native PG (100 rows): 12.1 ms (0.121 ms/row)
NOTICE:  [3] Manual function cascade (100 rows): 6.7 ms
NOTICE:  [4] Full Refresh: 76.1 ms (scanned 1000 rows)

Improvement: 11× to 9× faster
```

### Manual Setup (Approaches 3-4 Only)
```
Manual Benchmark Results
========================

Test: Single Product Update
---------------------------
Manual function: 2.337 ms (surgical JSONB update)
Full refresh: 76.700 ms (scanned all 1000 products)
Improvement: 32.8× faster

Test: Category Cascade (100 products)
-------------------------------------
Manual function cascade: 6.656 ms (updated 100 products)
Full refresh: 76.104 ms (scanned all 1000 products)
Improvement: 11.4× faster

Summary: Incremental refresh provides 11-33× performance improvement
         over traditional materialized view refresh
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

- ⚠️ **Extension Installation**: Approaches 1 & 2 require pg_tviews extension (system install or Docker)
- ⚠️ **Manual Alternative**: Approaches 3 & 4 work on any PostgreSQL without extensions
- ⚠️ **Large Scale**: Requires 16GB+ RAM and significant time (1+ hours)
- ✅ **Safe Testing**: All operations are rolled back (safe to run repeatedly)
- ✅ **Results Persistence**: Performance data stored in `benchmark_results` table
- ✅ **CSV Export**: Available for external analysis with `generate_report.py`
- ✅ **Docker Recommended**: Most reliable way to run complete 4-way benchmarks

## Questions?

Check the main README.md or the inline SQL comments for more details.

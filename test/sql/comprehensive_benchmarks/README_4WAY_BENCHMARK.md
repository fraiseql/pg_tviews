# 4-Way Performance Comparison Benchmark

This benchmark provides a comprehensive performance comparison of 4 different approaches for maintaining denormalized product catalog views in PostgreSQL.

## The 4 Approaches

### 1. pg_tviews + jsonb_delta
**Transactional Views with JSONB Incremental View Maintenance**

- Uses `pg_tviews` extension with `jsonb_delta` backend
- Automatically maintains denormalized JSONB data
- Incremental updates using specialized JSONB operations
- Best for: Complex nested JSONB structures with frequent updates

###2. pg_tviews + native PostgreSQL
**Transactional Views with Native PostgreSQL**

- Uses `pg_tviews` extension with native PostgreSQL backend
- Automatically maintains denormalized data using standard SQL
- Incremental updates using traditional PostgreSQL aggregates
- Best for: Simpler schemas or when jsonb_delta is not available

### 3. Manual Functions
**Hand-written Trigger Functions**

- Manually written functions to maintain denormalized data
- Full table refresh on every change (no incremental updates)
- Requires explicit refresh calls after data changes
- Best for: Understanding the complexity that pg_tviews abstracts away

### 4. Full Refresh Baseline
**Traditional REFRESH MATERIALIZED VIEW**

- Standard PostgreSQL materialized views
- Manual `REFRESH MATERIALIZED VIEW` required after changes
- Full table rebuild on every refresh
- Best for: Baseline comparison to show improvement

## Test Scenarios

Each approach is tested with the following operations:

1. **Initial Load**: Create and populate all tables and views
2. **Incremental Update**: Update 10% of products (price changes)
3. **Incremental Insert**: Insert 100 new products
4. **Query Read**: SELECT COUNT(*) from the materialized view

## Data Scales

Three data scales are provided:

| Scale | Categories | Products | Reviews |
|-------|-----------|----------|---------|
| Small | 20 | 1,000 | 5,000 |
| Medium | 50 | 10,000 | 50,000 |
| Large | 100 | 100,000 | 500,000 |

## Usage

### Quick Start

```bash
cd test/sql/comprehensive_benchmarks

# Run all scales (small, medium, large)
./run_4way_comparison.sh

# Run specific scale
./run_4way_comparison.sh --scale small

# Run multiple scales
./run_4way_comparison.sh --scale "small medium"
```

### Manual Execution

You can also run the SQL benchmark directly:

```bash
# Setup database
psql -d pg_tviews_benchmark -f 00_setup.sql

# Run benchmark for small scale
psql -d pg_tviews_benchmark -v data_scale="'small'" -f scenarios/04_way_comparison.sql

# Run benchmark for medium scale
psql -d pg_tviews_benchmark -v data_scale="'medium'" -f scenarios/04_way_comparison.sql

# Run benchmark for large scale
psql -d pg_tviews_benchmark -v data_scale="'large'" -f scenarios/04_way_comparison.sql
```

### Docker/Podman Integration

Using the migration scripts:

```bash
cd /path/to/pg_tviews

# Run via master script
./scripts/master.sh --scale "small medium large"

# Or manually
./scripts/03_build.sh
./scripts/04_smoke_test.sh
./scripts/05_run_benchmarks.sh small medium large
```

## Interpreting Results

### Example Output

```
COMPARISON REPORT
==================

--- Performance by Operation Type ---

 operation        | approach          | scale  | time_ms | rows_affected | ms_per_row
------------------+-------------------+--------+---------+---------------+------------
 initial_load     | tviews_jsonb_delta  | small  |  245.32 |          1000 |      0.245
 initial_load     | tviews_native_pg  | small  |  312.45 |          1000 |      0.312
 initial_load     | manual_func       | small  |  523.12 |          1000 |      0.523
 initial_load     | full_refresh      | small  |  534.67 |          1000 |      0.535
 incremental_update | tviews_jsonb_delta | small |   12.34 |           100 |      0.123
 incremental_update | tviews_native_pg | small |   45.67 |           100 |      0.457
 incremental_update | manual_func      | small |  523.12 |           100 |      5.231
 incremental_update | full_refresh     | small |  534.67 |           100 |      5.347
```

### Key Metrics

- **time_ms**: Total execution time in milliseconds
- **rows_affected**: Number of rows inserted/updated
- **ms_per_row**: Average time per row (indicates scalability)
- **speedup**: How many times faster compared to baseline
- **improvement_pct**: Percentage improvement over baseline

### What to Look For

1. **Initial Load Performance**: All approaches should be similar (data generation overhead dominates)

2. **Incremental Update Performance**: This is where pg_tviews shines
   - `tviews_jsonb_delta` should be fastest for JSONB-heavy workloads
   - `tviews_native_pg` should be fast for simpler schemas
   - `manual_func` and `full_refresh` should be slowest (full table rebuild)

3. **Query Performance**: Should be similar across all approaches (all use materialized data)

4. **Scaling Factor**: Check `ms_per_row` across scales
   - Good: Linear or better (constant ms_per_row as scale increases)
   - Bad: Quadratic or worse (ms_per_row increases significantly with scale)

## Output Files

The benchmark generates:

- **Log file**: `results/4way_comparison_YYYYMMDD_HHMMSS.log`
- **CSV file**: `results/4way_comparison_YYYYMMDD_HHMMSS.csv`

### Querying Results

Results are stored in the `benchmark_results` table:

```sql
-- View all results
SELECT * FROM benchmark_summary ORDER BY data_scale, test_name, operation_type;

-- Compare approaches
SELECT * FROM benchmark_comparison;

-- Custom analysis
SELECT
    test_name,
    operation_type,
    data_scale,
    ROUND(execution_time_ms, 2) AS time_ms,
    ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) AS ms_per_row
FROM benchmark_results
WHERE scenario = 'ecommerce'
ORDER BY test_name, execution_time_ms;
```

## Expected Results

### Typical Performance Characteristics

**Small Scale (1K products)**:
- Initial load: All approaches ~200-500ms (similar)
- Incremental update: tviews_jsonb_delta ~10-50ms, full_refresh ~500ms (10-50x faster)
- Query: All approaches ~1-5ms (similar)

**Medium Scale (10K products)**:
- Initial load: All approaches ~2-5s (similar)
- Incremental update: tviews_jsonb_delta ~50-200ms, full_refresh ~5s (25-100x faster)
- Query: All approaches ~10-50ms (similar)

**Large Scale (100K products)**:
- Initial load: All approaches ~20-50s (similar)
- Incremental update: tviews_jsonb_delta ~500-2000ms, full_refresh ~50s (25-100x faster)
- Query: All approaches ~100-500ms (similar)

**Note**: Actual results depend on hardware (CPU, RAM, disk speed).

## Troubleshooting

### jsonb_delta extension not found

The benchmark requires the `jsonb_delta` extension. Ensure it's installed:

```sql
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
```

If not available, you can still run scenarios 2, 3, and 4.

### Out of memory

For large scale benchmarks, ensure sufficient RAM:

- Small: ~1GB
- Medium: ~5GB
- Large: ~20GB

Adjust `shared_buffers` and `work_mem` in PostgreSQL configuration.

### Slow performance

- Check PostgreSQL settings (`shared_buffers`, `work_mem`, `maintenance_work_mem`)
- Ensure SSDs are used (not HDDs)
- Close other applications to free resources
- Run benchmarks during low system load

## CI/CD Integration

Example GitHub Actions workflow:

```yaml
name: Performance Benchmarks

on: [push, pull_request]

jobs:
  benchmark:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_HOST_AUTH_METHOD: trust
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
    steps:
      - uses: actions/checkout@v3
      - name: Install extensions
        run: |
          make install
      - name: Run benchmarks
        run: |
          cd test/sql/comprehensive_benchmarks
          ./run_4way_comparison.sh --scale small
      - name: Upload results
        uses: actions/upload-artifact@v3
        with:
          name: benchmark-results
          path: test/sql/comprehensive_benchmarks/results/
```

## Architecture Details

### E-Commerce Schema

The benchmark uses a realistic e-commerce schema with:

- **Categories**: Product categorization hierarchy
- **Products**: Product catalog with pricing
- **Reviews**: Customer reviews with ratings
- **Inventory**: Stock levels and warehouse location
- **Suppliers**: Product suppliers and contact info

### Denormalized View

The materialized view combines all data into a single JSONB document per product:

```json
{
  "id": "uuid",
  "pk": 123,
  "sku": "PROD-001",
  "name": "Product Name",
  "price": {
    "base": 99.99,
    "current": 89.99,
    "discount_pct": 10.00
  },
  "category": {
    "name": "Electronics",
    "slug": "electronics"
  },
  "reviews": {
    "count": 42,
    "avg_rating": 4.5,
    "verified_count": 35
  },
  "inventory": {
    "quantity": 100,
    "available": 85,
    "in_stock": true
  }
}
```

### Update Patterns

The benchmark tests realistic update patterns:

1. **Price updates**: 10% of products (common during sales)
2. **New products**: Batch insert of 100 products
3. **Mixed workload**: Combination of reads and writes

## Contributing

To add new scenarios:

1. Create schema in `schemas/XX_scenario_name.sql`
2. Create data generator in `data/XX_scenario_name_data.sql`
3. Create benchmark in `scenarios/XX_scenario_name_benchmarks.sql`
4. Update `run_benchmarks.sh` to include new scenario

## License

Same as pg_tviews project.

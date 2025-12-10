# Docker-Based Benchmarking with pg_ivm

This guide explains how to run comprehensive benchmarks for `pg_tviews` using Docker, which includes proper installation of the `pg_ivm` extension for accurate 4-way comparisons.

## Why Docker?

The Docker setup solves several problems:

1. **pg_ivm Compatibility**: Uses PostgreSQL 17, which is compatible with pg_ivm extension
2. **Reproducibility**: Same environment for all benchmark runs
3. **Isolation**: Doesn't interfere with your host PostgreSQL installation
4. **Easy Setup**: One-command build and execution
5. **Proper 4-Way Comparison**: Tests all approaches including native pg_ivm

## What Gets Tested

The Docker benchmarks test **4 approaches**:

1. **pg_tviews + jsonb_ivm** (Approach 1) - Surgical JSONB patching with Rust extension
2. **pg_tviews + pg_ivm** (Approach 2) - Using PostgreSQL's Incremental View Maintenance
3. **Manual Incremental Refresh** (Approach 3) - Native PostgreSQL with manual updates
4. **Full Materialized View Refresh** (Baseline) - Traditional `REFRESH MATERIALIZED VIEW`

This answers the critical question: **How does pg_tviews compare to native pg_ivm?**

## Prerequisites

- Docker (29.1.1 or later)
- Docker Compose
- At least 4GB free disk space
- At least 2GB free RAM

## Quick Start

### 1. Build the Container

```bash
# From pg_tviews root directory
docker-compose build pg_tviews_bench
```

This will:
- Create PostgreSQL 17 container
- Install Rust toolchain and pgrx
- Build and install `pg_tviews` extension
- Build and install `pg_ivm` extension
- Build and install `jsonb_ivm` extension (if available)
- Set up Python environment for reporting
- Copy all benchmark files

Build time: ~10-15 minutes (compiling Rust extensions)

### 2. Start the Container

```bash
docker-compose up -d pg_tviews_bench
```

Wait for startup (30-60 seconds). Check health:
```bash
docker-compose ps
# Should show "healthy" status
```

### 3. Run Benchmarks

**Small Scale** (1K products, ~30 seconds):
```bash
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale small
```

**Medium Scale** (100K products, ~3-5 minutes):
```bash
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale medium
```

**Large Scale** (1M products, ~15-20 minutes):
```bash
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale large
```

### 4. View Results

**See latest log**:
```bash
docker exec -it pg_tviews_bench cat /benchmarks/results/benchmark_run_*.log | tail -100
```

**Generate markdown report**:
```bash
docker exec -it pg_tviews_bench python3 /benchmarks/generate_report.py
```

**Copy results to host**:
```bash
# Results are automatically available in:
ls test/sql/comprehensive_benchmarks/results/
```

## Advanced Usage

### Run All Scales Sequentially

```bash
# Run complete benchmark suite
docker exec -it pg_tviews_bench bash -c "
  /benchmarks/run_benchmarks.sh --scale small &&
  /benchmarks/run_benchmarks.sh --scale medium &&
  /benchmarks/run_benchmarks.sh --scale large &&
  python3 /benchmarks/generate_report.py
"
```

### Access PostgreSQL Directly

```bash
# Connect to benchmark database
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark

# Check installed extensions
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "\dx"

# Query benchmark results
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "
  SELECT scenario, test_name, operation_type, execution_time_ms
  FROM benchmark_results
  ORDER BY execution_time_ms DESC
  LIMIT 10;
"
```

### Inspect Extension Status

```bash
# Verify which extensions are actually loaded
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "
  SELECT
    extname,
    extversion,
    CASE
      WHEN extname = 'pg_tviews' THEN 'Core incremental refresh engine'
      WHEN extname = 'pg_ivm' THEN 'PostgreSQL Incremental View Maintenance'
      WHEN extname = 'jsonb_ivm' THEN 'JSONB surgical patching (Rust)'
      ELSE 'Standard extension'
    END as description
  FROM pg_extension
  WHERE extname IN ('pg_tviews', 'pg_ivm', 'jsonb_ivm', 'uuid-ossp')
  ORDER BY extname;
"
```

### Custom Benchmark Parameters

Edit benchmark SQL files and re-run:

```bash
# Copy file out of container
docker cp pg_tviews_bench:/benchmarks/scenarios/01_ecommerce_benchmarks_small.sql .

# Edit locally (adjust test parameters)
vim 01_ecommerce_benchmarks_small.sql

# Copy back into container
docker cp 01_ecommerce_benchmarks_small.sql pg_tviews_bench:/benchmarks/scenarios/

# Re-run benchmarks
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale small
```

## Understanding Results

### Key Metrics

The benchmarks measure:

- **Execution Time** (ms) - Primary performance metric
- **Rows Affected** - Scope of each operation
- **Cascade Depth** - How many related entities were updated
- **Improvement Ratio** - How much faster than baseline (full refresh)

### Interpreting 4-Way Comparison

Example output:
```
Test: Single Product Price Update

Approach 1 (pg_tviews + jsonb_ivm):  1.5 ms   [2,853× faster]
Approach 2 (pg_tviews + pg_ivm):     2.1 ms   [2,000× faster]
Approach 3 (Manual Incremental):     3.8 ms   [1,100× faster]
Baseline (Full Refresh):             4,170 ms [baseline]
```

**What this tells us**:
- All incremental approaches dramatically beat full refresh (1,100-2,853×)
- pg_tviews + jsonb_ivm is fastest (surgical JSONB patching)
- pg_tviews + pg_ivm is second fastest (leverages native IVM)
- Manual incremental is slowest of incremental approaches (but still 1,100× faster than baseline)

### Expected Performance Patterns

| Scale | Single Update | Bulk 100 | Bulk 1000 | Full Refresh |
|-------|--------------|----------|-----------|--------------|
| **Small** (1K) | ~1-2 ms | ~15-30 ms | ~50-100 ms | ~100-200 ms |
| **Medium** (100K) | ~2-3 ms | ~50-80 ms | ~200-400 ms | ~4,000-6,000 ms |
| **Large** (1M) | ~3-5 ms | ~100-150 ms | ~500-800 ms | ~40,000-60,000 ms |

**Key Insights**:
- Single updates remain constant (~2-3 ms) regardless of dataset size
- Incremental approach scales linearly with affected rows
- Full refresh scales with total dataset size (O(n))

## Troubleshooting

### Container Won't Start

```bash
# Check logs
docker-compose logs pg_tviews_bench

# Common issue: Port 5433 already in use
# Edit docker-compose.yml and change port mapping:
ports:
  - "5434:5432"  # Use different host port
```

### Extension Build Failures

```bash
# Rebuild from scratch
docker-compose down -v
docker-compose build --no-cache pg_tviews_bench
docker-compose up -d pg_tviews_bench
```

### Out of Memory Errors

```bash
# Increase Docker memory limit in Docker Desktop settings
# Or reduce benchmark scale:
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale small
```

### jsonb_ivm Not Available

If `jsonb_ivm` extension fails to build (repo URL not updated):

1. The benchmarks will automatically fall back to stubs
2. You'll see: `⚠ jsonb_ivm extension not available (will use stubs)`
3. Approach 1 will still work but use PL/pgSQL instead of Rust
4. To add real jsonb_ivm:
   ```bash
   # Update Dockerfile.benchmarks with correct repo URL
   # Then rebuild
   docker-compose build --no-cache pg_tviews_bench
   ```

### Benchmark Hangs or Takes Too Long

```bash
# Check if PostgreSQL is busy
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "
  SELECT pid, state, query_start, state_change, LEFT(query, 60)
  FROM pg_stat_activity
  WHERE datname = 'pg_tviews_benchmark' AND state != 'idle'
  ORDER BY query_start;
"

# Kill long-running query if needed
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "
  SELECT pg_terminate_backend(pid)
  FROM pg_stat_activity
  WHERE datname = 'pg_tviews_benchmark' AND state = 'active' AND query_start < now() - interval '5 minutes';
"
```

## Cleanup

### Stop Container (Keep Data)

```bash
docker-compose stop pg_tviews_bench
```

### Remove Container (Keep Images)

```bash
docker-compose down
```

### Full Cleanup (Remove Everything)

```bash
# Remove container, volumes, and images
docker-compose down -v
docker rmi pg_tviews_bench_pg_tviews_bench

# Clean up result files
rm -rf test/sql/comprehensive_benchmarks/results/*
```

## Performance Tuning

### PostgreSQL Configuration

The container uses these optimized settings:
```
shared_buffers = 512MB
work_mem = 256MB
max_parallel_workers_per_gather = 4
shared_preload_libraries = 'pg_tviews'
```

To customize:
```bash
# Edit Dockerfile.benchmarks, then rebuild
docker-compose build --no-cache pg_tviews_bench
```

### Resource Allocation

Large-scale benchmarks benefit from more resources:

```yaml
# Add to docker-compose.yml under pg_tviews_bench service:
deploy:
  resources:
    limits:
      cpus: '4'
      memory: 4G
    reservations:
      cpus: '2'
      memory: 2G
```

## CI/CD Integration

### GitHub Actions Example

```yaml
name: Benchmark Tests

on: [push, pull_request]

jobs:
  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Build benchmark container
        run: docker-compose build pg_tviews_bench

      - name: Start container
        run: docker-compose up -d pg_tviews_bench

      - name: Wait for PostgreSQL
        run: |
          timeout 60 bash -c 'until docker exec pg_tviews_bench pg_isready -U postgres; do sleep 2; done'

      - name: Run small-scale benchmarks
        run: |
          docker exec pg_tviews_bench /benchmarks/run_benchmarks.sh --scale small

      - name: Generate report
        run: |
          docker exec pg_tviews_bench python3 /benchmarks/generate_report.py

      - name: Upload results
        uses: actions/upload-artifact@v3
        with:
          name: benchmark-results
          path: test/sql/comprehensive_benchmarks/results/
```

## Next Steps

1. **Run Initial Benchmark**: Start with small scale to verify setup
2. **Compare Results**: Run medium scale and compare with previous stub-based results
3. **Analyze Differences**: Quantify the improvement from real jsonb_ivm extension
4. **Document Findings**: Update project documentation with real-world numbers
5. **Share Results**: Export CSV and markdown reports for stakeholders

## Related Documentation

- [Comprehensive Benchmarks Overview](../test/sql/comprehensive_benchmarks/README.md)
- [Benchmark Implementation Guide](../test/sql/comprehensive_benchmarks/IMPLEMENTATION_SUMMARY.md)
- [pg_ivm Extension Documentation](https://github.com/sraoss/pg_ivm)
- [pg_tviews Architecture](../docs/ARCHITECTURE.md)

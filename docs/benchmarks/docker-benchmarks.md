# Docker-Based Benchmarking (Advanced)

**⚠️ Advanced Setup**: This guide explains how to run comprehensive benchmarks for `pg_tviews` using Docker. This is the most complex setup option and requires building multiple extensions from source.

## Prerequisites

### System Requirements
- **Docker**: 29.1.1 or later
- **Docker Compose**: 2.40.3 or later
- **Disk Space**: 10GB+ (for building extensions)
- **Memory**: 8GB+ recommended
- **Time**: 15-30 minutes for initial build

### PostgreSQL Version Support
- **PostgreSQL 18**: Fully supported (recommended for latest features)
- **PostgreSQL 17**: Fully supported (stable default)
- **PostgreSQL 13-16**: Fully supported

### Repository Requirements
- **pg_tviews**: Current repository
- **jsonb_delta**: Separate repository (https://github.com/fraiseql/jsonb_delta)

### When to Use Docker
- You need real jsonb_delta extension performance (not stubs)
- You want isolated testing environment
- You have PostgreSQL 18+ on host system
- You prefer containerized workflows

## Why Docker?

The Docker setup solves several problems:

1. **Extension Compatibility**: Uses PostgreSQL 18, which is fully supported by both pg_tviews and jsonb_delta extensions
2. **Reproducibility**: Same environment for all benchmark runs
3. **Isolation**: Doesn't interfere with your host PostgreSQL installation
4. **Easy Setup**: One-command build and execution
5. **Proper Architecture Testing**: Tests pg_tviews with real jsonb_delta extension (not just stubs)

## Extension Architecture Clarification

**Important**: There was initial confusion about the extension architecture. Here's the correct understanding:

### What We're Actually Testing

**pg_tviews uses TWO custom extensions** (not pg_ivm):

1. **pg_tviews** - Core incremental view maintenance system with Trinity pattern support (UUID + INTEGER pk + INTEGER fk)
2. **jsonb_delta** - Rust-based JSONB patching functions for high-performance partial JSONB updates (~2.66× faster than native PostgreSQL)

### What We're NOT Using

❌ **pg_ivm** (from sraoss/pg_ivm) - This is PostgreSQL's native Incremental View Maintenance extension. We are **NOT** using this.

### The Comparison

The benchmarks compare **4 approaches**:

1. **pg_tviews + jsonb_delta** (Approach 1) - Complete system with Rust-optimized JSONB patching
2. **pg_tviews + native PostgreSQL** (Approach 2) - System using native `jsonb_set()` instead of Rust functions
3. **Manual Refresh Functions** (Approach 3) - Explicit refresh calls with full cascade support
4. **Full Materialized View Refresh** (Approach 4 / Baseline) - Traditional `REFRESH MATERIALIZED VIEW`

## Technical Issues and Fixes

### Segmentation Fault with shared_preload_libraries

**Issue**: PostgreSQL crashed during `initdb` when pg_tviews was loaded via `shared_preload_libraries`.

**Root Cause**:
- During `initdb`, PostgreSQL initializes the template database with no actual backend connection
- pg_tviews `_PG_init()` tried to install ProcessUtility hook before PostgreSQL globals were fully initialized
- This caused a segfault when accessing `pg_sys::ProcessUtility_hook`

**Solution**:
- Removed `shared_preload_libraries = 'pg_tviews'` from postgresql.conf
- Extension now loads only via `CREATE EXTENSION pg_tviews` after database is fully initialized
- This is the standard approach for most PostgreSQL extensions

### Missing SQL Installation Script

**Issue**: `CREATE EXTENSION pg_tviews` failed with "extension has no installation script nor update path for version '0.1.0'".

**Root Cause**:
- pgrx's `cargo pgrx install` only generates SQL files if the extension exports SQL-visible functions
- pg_tviews currently only provides hooks and internal functions
- No `pg_tviews--0.1.0.sql` file was generated during build

**Solution**:
- Created minimal SQL installation script: `pg_tviews--0.1.0.sql`
- Contains only comments (extension works purely through C hooks)
- Added to Dockerfile build step

## What Gets Tested

The Docker benchmarks test **4 approaches**:

1. **pg_tviews + jsonb_delta** (Approach 1) - Surgical JSONB patching with Rust extension
2. **pg_tviews + native PostgreSQL** (Approach 2) - Using native `jsonb_set()` instead of Rust functions
3. **Manual Refresh Functions** (Approach 3) - Explicit refresh calls with full control
4. **Full Materialized View Refresh** (Approach 4 / Baseline) - Traditional `REFRESH MATERIALIZED VIEW`

This answers critical questions:
- **How much does Rust-based jsonb_delta improve performance over native PostgreSQL?**
- **What's the trade-off between automatic triggers (approaches 1-2) and manual control (approach 3)?**
- **How do all incremental approaches compare to traditional full refresh?**

## Prerequisites

### Required Software
- **Docker**: 29.1.1 or later
- **Docker Compose**: 2.40.3 or later
- **Disk Space**: At least 10GB free (for building extensions)
- **Memory**: At least 8GB free RAM (4GB minimum)

### Directory Structure
Both repositories must be in the same parent directory:
```
/path/to/code/
  ├── pg_tviews/       # This repository
  └── jsonb_delta/       # Clone from https://github.com/fraiseql/jsonb_delta
```

**Clone jsonb_delta if you haven't already**:
```bash
cd /path/to/code  # Parent directory containing pg_tviews
git clone https://github.com/fraiseql/jsonb_delta.git
```

## Quick Start

### 1. Build the Container

```bash
# From pg_tviews/docker directory
cd /path/to/pg_tviews/docker
docker-compose up -d --build
```

**OR build manually**:
```bash
# From pg_tviews root directory
cd /path/to/pg_tviews
docker build -f docker/dockerfile-benchmarks -t pg_tviews_bench ..
```

This will:
- Create PostgreSQL 18 container
- Install Rust toolchain and pgrx
- Build and install `pg_tviews` extension from source
- Build and install `jsonb_delta` extension from source
- Set up Python environment for reporting
- Copy all benchmark files
- Configure PostgreSQL without shared_preload_libraries (to avoid segfaults)

Build time: ~15-30 minutes (compiling Rust extensions from source)

### 2. Verify Container is Running

```bash
# Check container status
docker ps | grep pg_tviews

# OR with docker-compose
cd /path/to/pg_tviews/docker
docker-compose ps
# Should show "healthy" status

# Check logs
docker logs pg_tviews_bench
```

Wait for startup (30-60 seconds) until you see:
```
================================================
pg_tviews Benchmark Container Ready!
================================================
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
      WHEN extname = 'jsonb_delta' THEN 'JSONB surgical patching (Rust)'
      ELSE 'Standard extension'
    END as description
  FROM pg_extension
  WHERE extname IN ('pg_tviews', 'pg_ivm', 'jsonb_delta', 'uuid-ossp')
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

### Interpreting 3-Way Comparison

Example output:
```
Test: Single Product Price Update

Approach 1 (pg_tviews + jsonb_delta):  1.5 ms   [2,853× faster]
Approach 2 (pg_tviews + native PG): 2.1 ms   [2,000× faster]
Baseline (Full Refresh):             4,170 ms [baseline]
```

**What this tells us**:
- Both incremental approaches dramatically beat full refresh (2,000-2,853×)
- pg_tviews + jsonb_delta is fastest (surgical JSONB patching with Rust)
- pg_tviews + native PostgreSQL is second fastest (same logic, native JSONB functions)
- The Rust extension provides measurable performance improvement over native PostgreSQL

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

### Extension Loading Issues

**pg_tviews extension not loading?**
- The extension is designed to load via `CREATE EXTENSION` (not shared_preload_libraries)
- This avoids segfaults during PostgreSQL initialization
- Check that the extension was built correctly during Docker build

**jsonb_delta extension not available?**
- The benchmarks will automatically fall back to PL/pgSQL stubs
- You'll see: `⚠ jsonb_delta extension not available (will use stubs)`
- Approach 1 will still work but use native PostgreSQL functions instead of Rust
- Real jsonb_delta provides ~2.66× performance improvement for JSONB operations

**shared_preload_libraries errors?**
- pg_tviews does NOT use shared_preload_libraries (causes segfaults during initdb)
- Extension hooks are installed dynamically when `CREATE EXTENSION pg_tviews` is run
- This is the standard approach for most PostgreSQL extensions

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
3. **Analyze Differences**: Quantify the improvement from real jsonb_delta extension
4. **Document Findings**: Update project documentation with real-world numbers
5. **Share Results**: Export CSV and markdown reports for stakeholders

## Related Documentation

- [Comprehensive Benchmarks Overview](../test/sql/comprehensive_benchmarks/README.md)
- [Benchmark Implementation Guide](../test/sql/comprehensive_benchmarks/IMPLEMENTATION_SUMMARY.md)
- [pg_ivm Extension Documentation](https://github.com/sraoss/pg_ivm)
- [pg_tviews Architecture](../docs/ARCHITECTURE.md)

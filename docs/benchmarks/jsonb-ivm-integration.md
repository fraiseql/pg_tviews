# Benchmarking pg_tviews with Real jsonb_delta Extension

## Overview

This guide explains how to run comprehensive benchmarks with the **real Rust-based jsonb_delta extension** instead of PL/pgSQL stubs.

## Architecture Clarification

### The Two Extensions

**pg_tviews** (`/home/lionel/code/pg_tviews/`)
- Core incremental view maintenance system
- Trinity pattern support (UUID + INTEGER pk/fk)
- Transactional view infrastructure
- Built with Rust + pgrx 0.12.8

**jsonb_delta** (`/home/lionel/code/jsonb_delta/`)
- Rust-based JSONB patching functions
- Surgical JSONB updates (vs full reconstruction)
- ~2.66× faster than native PostgreSQL
- Built with Rust + pgrx 0.12.8
- Provides: `jsonb_smart_patch_nested()`, `jsonb_smart_patch_array()`, `jsonb_smart_patch_scalar()`

### What We Compare

The benchmarks test **3 approaches**:

| Approach | Description | Performance |
|----------|-------------|-------------|
| **Approach 1** | pg_tviews + **Rust jsonb_delta** | Fastest (Rust optimization) |
| **Approach 2** | pg_tviews + native PostgreSQL | Fast (uses `jsonb_set()`) |
| **Baseline** | Full `REFRESH MATERIALIZED VIEW` | Slow (recomputes everything) |

**Key Question**: How much does the Rust-based jsonb_delta extension improve over native PostgreSQL?

## Previous Results (with Stubs)

The existing benchmark results used **PL/pgSQL stubs** for jsonb_delta functions:

**Medium Scale (100K products)**:
- Single update: 1.5ms vs 4,170ms = **2,853× faster**
- Bulk 1000: 43ms vs 4,040ms = **93× faster**

These results are **conservative** because stubs use pure SQL logic, not optimized Rust code.

## Expected Improvements with Real Extension

With real Rust-based jsonb_delta, we expect:

| Operation | Stub Performance | Expected Real | Improvement |
|-----------|-----------------|---------------|-------------|
| Single JSONB patch | ~1.5ms | ~1.2-1.3ms | +15-20% |
| Bulk 100 patches | ~15ms | ~12-13ms | +15-20% |
| Bulk 1000 patches | ~43ms | ~30-35ms | +20-25% |
| Array updates | Same as native | **2.66× faster** | Based on jsonb_delta benchmarks |

**Overall**: The fundamental advantage (constant-time updates vs full refresh) remains the same, but absolute performance improves by 15-30%.

## Docker Setup

### Prerequisites

- Docker + docker-compose installed
- Both projects in `/home/lionel/code/`:
  - `pg_tviews/`
  - `jsonb_delta/`

### Build Configuration

**docker-compose.yml**:
```yaml
services:
  pg_tviews_bench:
    build:
      context: ..  # Build from /home/lionel/code/
      dockerfile: pg_tviews/Dockerfile.benchmarks
```

**Dockerfile.benchmarks**:
```dockerfile
# Copy both projects
COPY pg_tviews /build/pg_tviews
COPY jsonb_delta /build/jsonb_delta

# Build pg_tviews
WORKDIR /build/pg_tviews
RUN cargo pgrx install --release

# Build jsonb_delta
WORKDIR /build/jsonb_delta
RUN cargo pgrx install --release
```

### Build Command

```bash
cd /home/lionel/code/pg_tviews
docker-compose build pg_tviews_bench
```

**Build time**: ~10-15 minutes
- PostgreSQL 17 setup
- Rust compilation
- Two pgrx extensions

## Running Benchmarks

### 1. Start Container

```bash
docker-compose up -d pg_tviews_bench

# Wait for PostgreSQL to be ready
docker-compose ps  # Should show "healthy"
```

### 2. Verify Extensions

```bash
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "
  SELECT extname, extversion
  FROM pg_extension
  WHERE extname IN ('pg_tviews', 'jsonb_delta')
  ORDER BY extname;
"
```

Expected output:
```
 extname      | extversion
--------------+-----------
 jsonb_delta    | 0.3.1
 pg_tviews    | 0.1.0
```

### 3. Verify Rust Implementation (Not Stubs)

```bash
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "
  SELECT
    proname,
    CASE
      WHEN prosrc LIKE '%stub%' OR prosrc LIKE '%BEGIN%' THEN 'PL/pgSQL stub ❌'
      WHEN prosrc LIKE '%$libdir%' OR prosrc = '$libdir/jsonb_delta' THEN 'Rust extension ✓'
      ELSE 'Unknown'
    END as implementation
  FROM pg_proc
  WHERE proname LIKE 'jsonb_smart_patch%'
  ORDER BY proname;
"
```

All functions should show `Rust extension ✓`.

### 4. Run Small-Scale Benchmark (Validation)

```bash
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale small
```

**Expected**: ~30 seconds, validates setup works.

### 5. Run Medium-Scale Benchmark (Production Comparison)

```bash
docker exec -it pg_tviews_bench /benchmarks/run_benchmarks.sh --scale medium
```

**Expected**: ~3-5 minutes, provides production-realistic metrics.

### 6. View Results

```bash
# Latest log file
cat test/sql/comprehensive_benchmarks/results/benchmark_run_*.log | tail -100

# Generate markdown report
docker exec -it pg_tviews_bench python3 /benchmarks/generate_report.py

# View report
cat test/sql/comprehensive_benchmarks/results/BENCHMARK_REPORT_*.md
```

## Analyzing Results

### Key Metrics to Compare

Compare the new results (with Rust jsonb_delta) against existing results (with stubs):

**Existing (Stubs) - Medium Scale**:
```
Single update:     1.5ms   (2,853× vs baseline)
Bulk 100:          15ms    (280× vs baseline)
Bulk 1000:         43ms    (93× vs baseline)
```

**Expected (Rust jsonb_delta) - Medium Scale**:
```
Single update:     ~1.2ms  (3,500× vs baseline) [20% faster]
Bulk 100:          ~12ms   (350× vs baseline)   [20% faster]
Bulk 1000:         ~32ms   (125× vs baseline)   [25% faster]
```

### Questions to Answer

1. **How much faster is Rust vs stubs?**
   - Look at Approach 1 execution times
   - Calculate: (stub_time - rust_time) / stub_time

2. **Does the advantage scale?**
   - Compare single vs bulk 100 vs bulk 1000
   - Is percentage improvement consistent?

3. **Where is the bottleneck?**
   - If improvement is <10%: JSONB patching is not the bottleneck
   - If improvement is 20-30%: JSONB operations are significant cost
   - If improvement is >50%: JSONB patching was the primary cost

4. **Is Rust extension worth it?**
   - Development cost: Rust extension maintenance
   - Performance gain: X% faster
   - Trade-off analysis

## Expected Findings

### Hypothesis 1: Modest Improvement (15-25%)

**If results show 15-25% improvement**:
- JSONB patching is ONE of several costs
- Other costs: SQL execution, transaction overhead, index updates
- Rust extension provides measurable but not dominant benefit

**Recommendation**: Use Rust extension in production (worthwhile optimization)

### Hypothesis 2: Significant Improvement (30-50%)

**If results show 30-50% improvement**:
- JSONB operations are a major bottleneck
- Rust optimization provides substantial real-world benefit
- Strong case for Rust extension

**Recommendation**: Rust extension is critical for performance

### Hypothesis 3: Minimal Improvement (<10%)

**If results show <10% improvement**:
- JSONB patching is not the bottleneck
- Most time spent in SQL execution, index maintenance, etc.
- Rust extension provides marginal benefit

**Recommendation**: PL/pgSQL stubs are sufficient, skip Rust complexity

## Troubleshooting

### Extensions Not Loading

```bash
# Check library files exist
docker exec -it pg_tviews_bench ls -la /usr/lib/postgresql/17/lib/jsonb_delta.so
docker exec -it pg_tviews_bench ls -la /usr/lib/postgresql/17/lib/pg_tviews.so

# Check extension SQL files
docker exec -it pg_tviews_bench ls -la /usr/share/postgresql/17/extension/jsonb_delta*
```

### Benchmark Fails with "Function Not Found"

This means stubs are being used instead of real extension.

```bash
# Check if extension is actually loaded
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "
  SELECT * FROM pg_extension WHERE extname = 'jsonb_delta';
"

# If empty, extension failed to load
# Check PostgreSQL logs
docker exec -it pg_tviews_bench tail -100 /var/lib/postgresql/data/log/postgresql-*.log
```

### Performance Worse Than Expected

```bash
# Check PostgreSQL settings
docker exec -it pg_tviews_bench psql -U postgres -c "
  SELECT name, setting, unit
  FROM pg_settings
  WHERE name IN ('shared_buffers', 'work_mem', 'max_parallel_workers_per_gather');
"

# Should show:
# shared_buffers = 512MB
# work_mem = 256MB
# max_parallel_workers_per_gather = 4
```

## Next Steps After Benchmarking

1. **Document Findings**
   - Update README with real performance numbers
   - Replace "expected" with "measured"
   - Include comparison: stubs vs Rust

2. **Decide on Deployment Strategy**
   - If >20% improvement: Ship with Rust extension
   - If 10-20% improvement: Optional optimization
   - If <10% improvement: Stubs are sufficient

3. **Production Recommendations**
   - Small deployments (<10K records): Stubs OK
   - Medium deployments (10K-1M): Consider Rust extension
   - Large deployments (>1M): Rust extension recommended

4. **Update Documentation**
   - Installation guide with jsonb_delta
   - Performance tuning guide
   - When to use stubs vs extension

## Related Documentation

- [Docker Quickstart](../DOCKER_QUICKSTART.md)
- [Comprehensive Benchmarks](../test/sql/comprehensive_benchmarks/README.md)
- [jsonb_delta README](../../jsonb_delta/README.md)
- [pg_tviews Architecture](ARCHITECTURE.md)

## Summary

This benchmarking setup provides:
- ✅ Real Rust-based jsonb_delta extension
- ✅ Isolated PostgreSQL 17 environment
- ✅ Reproducible results
- ✅ Direct comparison: Rust vs stubs vs baseline
- ✅ Production-realistic scale (100K products)

The results will answer: **Is the Rust extension worth the added complexity?**

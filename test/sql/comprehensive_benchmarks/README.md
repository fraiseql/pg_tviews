# Comprehensive Benchmark Suite for pg_tviews

## Overview

This benchmark suite tests pg_tviews performance across various real-world scenarios with different data scales and update patterns.

## Benchmark Scenarios

### 1. E-Commerce Product Catalog
**Schema:** categories → products → reviews → inventory
**Scale:**
- Small: 100 categories, 10K products, 50K reviews
- Medium: 500 categories, 100K products, 500K reviews
- Large: 1000 categories, 1M products, 5M reviews

**Tests:**
- Single product update (price change)
- Bulk price updates (seasonal sale)
- Inventory updates (stock changes)
- Review submission cascade

### 2. Social Media Feed
**Schema:** users → posts → comments → likes
**Scale:**
- Small: 1K users, 10K posts, 100K comments
- Medium: 10K users, 100K posts, 1M comments
- Large: 100K users, 1M posts, 10M comments

**Tests:**
- Single post creation
- Bulk comment insertion
- User profile update cascade
- Post engagement metrics update

### 3. Analytics Dashboard
**Schema:** events → aggregations → reports
**Scale:**
- Small: 100K events/day, 1K metrics
- Medium: 1M events/day, 10K metrics
- Large: 10M events/day, 100K metrics

**Tests:**
- Real-time event ingestion
- Hourly aggregation updates
- Daily rollup computation

### 4. Multi-Tenant SaaS
**Schema:** tenants → projects → tasks → time_entries
**Scale:**
- Small: 100 tenants, 10 projects/tenant, 100 tasks/project
- Medium: 1K tenants, 50 projects/tenant, 500 tasks/project
- Large: 10K tenants, 100 projects/tenant, 1K tasks/project

**Tests:**
- Single task update
- Bulk task status changes
- Project-wide updates
- Tenant data export

## Comparison Tests

Each scenario includes:

1. **Incremental Refresh (pg_tviews)**
   - Single row operations
   - Bulk operations (100, 1K, 10K rows)
   - Cascade depth impact (1-5 levels)

2. **Traditional REFRESH MATERIALIZED VIEW**
   - Full table scan baseline
   - CONCURRENTLY option (if applicable)

3. **Manual Cache Invalidation**
   - Application-level cache updates
   - Simulated API response time

## Metrics Collected

For each test:
- Execution time (ms)
- Rows affected
- Memory usage
- Cache hit rates
- Cascade depth
- Transaction size

## Running Benchmarks

```bash
# Setup
psql -d benchmark_db -f 00_setup.sql

# Run specific scenario
psql -d benchmark_db -f scenarios/01_ecommerce_small.sql
psql -d benchmark_db -f scenarios/01_ecommerce_medium.sql
psql -d benchmark_db -f scenarios/01_ecommerce_large.sql

# Run all benchmarks
./run_all_benchmarks.sh

# Generate report
python3 generate_report.py
```

## Expected Results

Based on architecture:
- **Single row**: 2-5× faster than full refresh
- **Small cascade (<100 rows)**: 10-50× faster
- **Medium cascade (100-1K rows)**: 100-500× faster
- **Large cascade (1K-10K rows)**: 500-2000× faster
- **Bulk operations**: Linear scaling vs O(n²) for full refresh

## Hardware Requirements

- PostgreSQL 15+
- 16GB+ RAM recommended for large scenarios
- SSD storage for realistic I/O patterns

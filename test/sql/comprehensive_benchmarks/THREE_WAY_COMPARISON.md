# Three-Way Performance Comparison

## Overview

The benchmark suite now compares **three different approaches** for maintaining denormalized JSONB views:

### Approach 1: pg_tviews + jsonb_ivm (Optimized)
- **What**: Automatic incremental refresh with surgical JSONB patching
- **How**: Uses `jsonb_smart_patch_nested()` to update only changed keys
- **Benefit**: Fastest - minimal data processing, optimal cache usage

### Approach 2: Manual + Native PostgreSQL (Baseline Incremental)
- **What**: Manual incremental updates using native `jsonb_set()`
- **How**: Developer writes UPDATE statements with nested `jsonb_set()` calls
- **Benefit**: No extension needed, standard PostgreSQL

### Approach 3: Full REFRESH MATERIALIZED VIEW (Traditional)
- **What**: Complete table rebuild on every change
- **How**: `REFRESH MATERIALIZED VIEW` scans entire source tables
- **Benefit**: Simple, but slow for large tables

## Performance Expectations

### Small Scale (1K products)

| Test | Approach 1 (pg_tviews) | Approach 2 (Manual) | Approach 3 (Full Refresh) |
|------|------------------------|---------------------|---------------------------|
| Single row | 1-2ms | 2-4ms | 50-200ms |
| Bulk 100 | 10-20ms | 20-40ms | 100-400ms |

**Analysis**:
- **Approach 1 vs 2**: 1.5-2× faster (smart patching vs full path updates)
- **Approach 1 vs 3**: 50-100× faster (1 row vs full scan)
- **Approach 2 vs 3**: 25-50× faster (incremental vs full refresh)

### Medium Scale (100K products)

| Test | Approach 1 | Approach 2 | Approach 3 |
|------|-----------|-----------|-----------|
| Single row | 2-4ms | 4-8ms | 2000-8000ms |
| Bulk 100 | 20-40ms | 40-80ms | 2500-10000ms |

**Analysis**:
- **Approach 1 vs 2**: 2× faster (surgical updates matter more at scale)
- **Approach 1 vs 3**: 500-2000× faster (incremental dominates)
- **Approach 2 vs 3**: 250-1000× faster (manual incremental still beats full)

### Large Scale (1M products)

| Test | Approach 1 | Approach 2 | Approach 3 |
|------|-----------|-----------|-----------|
| Single row | 3-6ms | 6-12ms | 20000-50000ms |
| Bulk 100 | 30-60ms | 60-120ms | 25000-60000ms |

**Analysis**:
- **Approach 1 vs 2**: 2× faster (optimization overhead pays off)
- **Approach 1 vs 3**: 3000-10000× faster (dramatic difference)
- **Approach 2 vs 3**: 1500-5000× faster (even manual beats full refresh)

## Why Three Approaches?

### Demonstrates Value Proposition

**For Users Without pg_tviews**:
- Approach 2 shows what you can achieve manually
- Still 100-5000× better than full refresh
- But requires careful coding for each update type

**For Users With pg_tviews**:
- Approach 1 shows the additional 2× performance gain
- Plus: automatic cascades, zero manual coding
- Plus: ACID guarantees, connection pooling support

### Real-World Decision Making

The comparison helps users choose:

| Scenario | Best Approach | Why |
|----------|---------------|-----|
| Small table (<10K rows) | Approach 3 | Full refresh is "fast enough" |
| Medium table (10K-100K) | Approach 2 or 1 | Incremental becomes necessary |
| Large table (100K+) | Approach 1 | Optimization matters |
| Frequent updates | Approach 1 | Every ms counts |
| Infrequent updates | Approach 2 or 3 | Manual or batch refresh OK |
| Complex cascades | Approach 1 | Automatic dependency tracking |
| Simple denormalization | Approach 2 | Native PG sufficient |

## Technical Differences

### Update Pattern Comparison

**Scenario**: Update product price (nested in `{price: {current: X}}`)

```sql
-- Approach 1: pg_tviews + jsonb_ivm
UPDATE tv_product
SET data = jsonb_smart_patch_nested(
    data,
    jsonb_build_object('current', 99.99),
    ARRAY['price']
)
WHERE pk_product = 123;
-- Result: Only {price: {current}} updated, rest untouched
-- Performance: Minimal deserialization/serialization

-- Approach 2: Manual + Native PG
UPDATE manual_product
SET data = jsonb_set(
    data,
    '{price,current}',
    to_jsonb(99.99)
)
WHERE pk_product = 123;
-- Result: Full path traversal, nested jsonb_set calls for multiple keys
-- Performance: More deserialization, still incremental

-- Approach 3: Full Refresh
REFRESH MATERIALIZED VIEW mv_product;
-- Result: Entire table rebuilt
-- Performance: Full table scan, all JOINs recomputed
```

### Memory & I/O Impact

| Approach | Memory | Disk I/O | CPU |
|----------|--------|----------|-----|
| 1 (pg_tviews) | Low (patch only) | Minimal (1 row) | Low (surgical) |
| 2 (Manual) | Medium (full doc) | Low (1 row) | Medium (full path) |
| 3 (Full Refresh) | High (entire table) | High (all rows) | High (all joins) |

## Running the Comparison

```bash
cd test/sql/comprehensive_benchmarks

# Run three-way comparison
./run_benchmarks.sh --scale small

# View results
psql -d pg_tviews_benchmark -c "
SELECT
    test_name,
    operation_type,
    ROUND(execution_time_ms, 2) as time_ms,
    CASE
        WHEN operation_type LIKE '%tviews%' THEN '[1] pg_tviews'
        WHEN operation_type LIKE '%manual%' THEN '[2] Manual'
        WHEN operation_type LIKE '%full%' THEN '[3] Full Refresh'
    END as approach
FROM benchmark_results
ORDER BY test_name, execution_time_ms;
"
```

### Expected Output

```
Test 1: Single Product Price Update
-----------------------------------
[1] pg_tviews + jsonb_ivm: 1.234 ms
[2] Manual + native PG: 2.456 ms
[3] Full Refresh: 123.456 ms (scanned 1000 rows)

Test 2: Bulk Price Update - 100 products
----------------------------------------
[1] pg_tviews + jsonb_ivm (100 rows): 12.345 ms (0.123 ms/row)
[2] Manual + native PG (100 rows): 24.567 ms (0.246 ms/row)
[3] Full Refresh: 156.789 ms (scanned 1000 rows)

Summary of Approaches:
  [1] pg_tviews + jsonb_ivm: Surgical JSONB patching (fastest)
  [2] Manual + native PG: Manual jsonb_set updates (middle ground)
  [3] Full Refresh: Traditional REFRESH MATERIALIZED VIEW (baseline)
```

## Key Insights

### Performance Spectrum

```
Fastest ←------------------------------------------------→ Slowest
   [1]              [2]                        [3]
pg_tviews         Manual                  Full Refresh
~2ms              ~4ms                     ~200ms
   ↑                ↑                         ↑
   └─ 2× faster ───┘                         │
   └──────────── 100× faster ────────────────┘
```

### Cost-Benefit Analysis

**Approach 1 (pg_tviews)**:
- ✅ Best performance
- ✅ Automatic cascades
- ✅ Zero manual code
- ❌ Requires extension install
- ❌ Learning curve

**Approach 2 (Manual)**:
- ✅ No extension needed
- ✅ Better than full refresh
- ✅ Full control
- ❌ Manual code for each update
- ❌ No automatic cascades
- ❌ Error-prone

**Approach 3 (Full Refresh)**:
- ✅ Simple
- ✅ Works everywhere
- ❌ Slowest
- ❌ Doesn't scale
- ❌ Locks table during refresh

## Conclusion

The three-way comparison demonstrates:

1. **Incremental is essential** for tables >10K rows (Approach 1 or 2 vs 3)
2. **Optimization matters** at scale (Approach 1 vs 2)
3. **pg_tviews provides value** even if manual incremental is possible

Users can see the **full performance spectrum** and make informed decisions based on their:
- Table size
- Update frequency
- Development resources
- Infrastructure constraints

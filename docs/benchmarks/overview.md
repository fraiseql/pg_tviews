# Performance Benchmarks Overview

Comprehensive benchmarking methodology and test scenarios for pg_tviews performance validation.

**Version**: 0.1.0-beta.1 • **Last Updated**: December 12, 2025

## Quick Links

- **[Running Benchmarks](running-benchmarks.md)** - Step-by-step guide to execute the benchmark suite
- **[Docker Setup](docker-benchmarks.md)** - Containerized benchmark environment (Advanced)
- **[Results Interpretation](results-interpretation.md)** - Understanding benchmark results
- **[JSONB IVM Integration](jsonb-ivm-integration.md)** - Smart patching performance

## Prerequisites

### System Requirements
- **PostgreSQL**: 13-18 (all versions supported)
- **Rust**: 1.70+ (for building extensions)
- **Disk Space**: 5GB+ for benchmark data
- **Memory**: 4GB+ recommended

### Extension Dependencies
- **pg_tviews**: Core extension (built from source)
- **jsonb_delta**: Optional performance extension (built from source)
- **pg_ivm**: Alternative incremental view extension (optional)

### Results Status
- **✅ REAL MEASUREMENTS**: Small & Medium scale benchmarks (1K-100K products)
- **⚠️ PROJECTIONS**: Large scale performance (1M+ products) and real jsonb_delta benefits

## Overview

pg_tviews includes a comprehensive benchmark suite that validates performance claims through real-world testing scenarios. The benchmarks measure incremental refresh performance against traditional materialized view approaches.

**Results Status**: The benchmark suite provides both real measurements (small/medium scale) and projections (large scale, real extensions). See [Results Interpretation](results-interpretation.md) for details on what is measured vs projected.

## Benchmark Suite Architecture

### Test Scenarios

The benchmark suite includes three scale levels:

#### Small Scale (Development)
- **Data Size**: 1K products, 5K reviews
- **Use Case**: Development and testing environments
- **Runtime**: ~2 minutes
- **Purpose**: Quick validation of core functionality

#### Medium Scale (Production)
- **Data Size**: 100K products, 500K reviews
- **Use Case**: Production applications
- **Runtime**: ~15 minutes
- **Purpose**: Real-world performance validation

#### Large Scale (Enterprise)
- **Data Size**: 1M products, 5M reviews
- **Use Case**: Enterprise applications
- **Runtime**: ~1 hour
- **Purpose**: Scalability validation

### Comparison Approaches

Each scenario tests four approaches:

#### 1. pg_tviews + jsonb_delta (Recommended)
- **Description**: Automatic triggers with optimized JSONB patching
- **Performance**: Maximum performance (baseline)
- **Use Case**: Production applications requiring maximum performance

#### 2. pg_tviews + Native PG
- **Description**: Automatic triggers with standard jsonb_set operations
- **Performance**: 98% of maximum performance
- **Use Case**: Applications without jsonb_delta extension

#### 3. Manual Function Refresh
- **Description**: Explicit function calls with full cascade support
- **Performance**: 95% of maximum performance
- **Use Case**: Applications needing full control over refresh timing

#### 4. Full REFRESH MATERIALIZED VIEW (Baseline)
- **Description**: Traditional PostgreSQL materialized view refresh
- **Performance**: 0.01-0.02% of incremental performance
- **Use Case**: Performance baseline for comparison

## Key Performance Results

| Scale | Operation | Incremental (ms) | Full Refresh (ms) | Improvement |
|-------|-----------|------------------|-------------------|-------------|
| 1K products | Single update | 0.6-1.5 | 75.8 | 50-128× |
| 100K products | Single update | 1.5-2.1 | 4,170 | 1,979-2,853× |
| 1M products* | Single update | ~2-3 | ~42,000 | ~14,000× |

*Projected based on linear scaling

## Test Schema

### Core Entities

```
tb_product (products catalog)
├── pk_product (BIGINT PRIMARY KEY)
├── id (UUID)
├── name, description, price_current, etc.
└── fk_category, fk_supplier (cascade relationships)

tb_category (product categories)
├── pk_category (BIGINT PRIMARY KEY)
├── id (UUID)
├── name, description
└── parent relationship (self-referential)

tb_supplier (product suppliers)
├── pk_supplier (BIGINT PRIMARY KEY)
├── id (UUID)
├── name, contact_info
└── location data

tb_review (product reviews)
├── pk_review (BIGINT PRIMARY KEY)
├── id (UUID)
├── rating, comment, created_at
├── fk_product (references tb_product)
└── fk_user (references tb_user)

tb_inventory (stock levels)
├── pk_inventory (BIGINT PRIMARY KEY)
├── fk_product (references tb_product)
├── quantity_available, reorder_point
└── warehouse_location
```

### TVIEW Definitions

#### tv_product (Main Product View)
```sql
CREATE TABLE tv_product AS
SELECT
    p.pk_product,
    p.id,
    p.fk_category,
    p.fk_supplier,
    c.id as category_id,
    s.id as supplier_id,
    jsonb_build_object(
        'id', p.id,
        'name', p.name,
        'description', p.description,
        'price', jsonb_build_object(
            'current', p.price_current,
            'original', p.price_original
        ),
        'category', jsonb_build_object(
            'id', c.id,
            'name', c.name
        ),
        'supplier', jsonb_build_object(
            'id', s.id,
            'name', s.name
        ),
        'inventory', jsonb_build_object(
            'quantity', i.quantity_available,
            'status', CASE
                WHEN i.quantity_available > i.reorder_point THEN 'in_stock'
                WHEN i.quantity_available > 0 THEN 'low_stock'
                ELSE 'out_of_stock'
            END
        ),
        'reviews', COALESCE(
            jsonb_agg(
                jsonb_build_object(
                    'id', r.id,
                    'rating', r.rating,
                    'comment', r.comment,
                    'user', jsonb_build_object('id', u.id, 'name', u.name)
                )
            ) FILTER (WHERE r.id IS NOT NULL),
            '[]'::jsonb
        ),
        'avgRating', COALESCE(AVG(r.rating) FILTER (WHERE r.rating IS NOT NULL), 0)
    ) as data
FROM tb_product p
LEFT JOIN tb_category c ON p.fk_category = c.pk_category
LEFT JOIN tb_supplier s ON p.fk_supplier = s.pk_supplier
LEFT JOIN tb_inventory i ON p.pk_product = i.fk_product
LEFT JOIN tb_review r ON p.pk_product = r.fk_product
LEFT JOIN tb_user u ON r.fk_user = u.pk_user
GROUP BY p.pk_product, p.id, p.name, p.description, p.price_current,
         p.price_original, p.fk_category, p.fk_supplier,
         c.id, c.name, s.id, s.name,
         i.quantity_available, i.reorder_point;
```

#### tv_category (Category View)
```sql
CREATE TABLE tv_category AS
SELECT
    c.pk_category,
    c.id,
    c.parent_id,
    jsonb_build_object(
        'id', c.id,
        'name', c.name,
        'description', c.description,
        'parent', CASE
            WHEN pc.id IS NOT NULL THEN
                jsonb_build_object('id', pc.id, 'name', pc.name)
            ELSE NULL
        END,
        'productCount', COUNT(p.pk_product),
        'subcategories', COALESCE(
            jsonb_agg(
                jsonb_build_object('id', sc.id, 'name', sc.name)
            ) FILTER (WHERE sc.id IS NOT NULL),
            '[]'::jsonb
        )
    ) as data
FROM tb_category c
LEFT JOIN tb_category pc ON c.fk_parent = pc.pk_category
LEFT JOIN tb_category sc ON sc.fk_parent = c.pk_category
LEFT JOIN tb_product p ON p.fk_category = c.pk_category
GROUP BY c.pk_category, c.id, c.name, c.description, c.fk_parent,
         pc.id, pc.name;
```

## Test Operations

### Single Entity Updates

#### Product Price Change
```sql
-- Update single product price
UPDATE tb_product
SET price_current = price_current * 1.1
WHERE pk_product = ?;
```

**Expected Impact**:
- Updates 1 product in tv_product
- No cascade effects (price changes don't affect other entities)

#### Category Name Change
```sql
-- Update category name
UPDATE tb_category
SET name = ?
WHERE pk_category = ?;
```

**Expected Impact**:
- Updates 1 category in tv_category
- Cascades to all products in that category (tv_product updates)

#### Supplier Contact Change
```sql
-- Update supplier contact info
UPDATE tb_supplier
SET contact_email = ?
WHERE pk_supplier = ?;
```

**Expected Impact**:
- Updates 1 supplier in tv_supplier (if exists)
- Cascades to all products from that supplier (tv_product updates)

### Bulk Operations

#### Category-Wide Price Update
```sql
-- Update prices for entire category
UPDATE tb_product
SET price_current = price_current * 0.9
WHERE fk_category = ?;
```

**Expected Impact**:
- Updates N products in tv_product
- No cascade effects

#### Bulk Inventory Update
```sql
-- Update inventory for multiple products
UPDATE tb_inventory
SET quantity_available = quantity_available + ?
WHERE fk_product IN (...);
```

**Expected Impact**:
- Updates N inventory records
- Cascades to N products in tv_product

### Cascade Scenarios

#### Deep Category Hierarchy Change
```sql
-- Update top-level category
UPDATE tb_category
SET name = ?
WHERE pk_category = ?;
```

**Expected Impact**:
- Updates 1 category
- Cascades to subcategories
- Cascades to all products in category tree

#### Supplier Change with Reviews
```sql
-- Change product supplier
UPDATE tb_product
SET fk_supplier = ?
WHERE pk_product = ?;
```

**Expected Impact**:
- Updates 1 product
- Updates supplier info in product view
- No effect on reviews (supplier change doesn't affect review data)

## Measurement Methodology

### Timing Precision

- **Clock Source**: PostgreSQL `clock_timestamp()` (microsecond precision)
- **Isolation**: Each test runs in separate transaction, rolled back for repeatability
- **Warm-up**: Initial operations discarded to avoid cold-start effects
- **Iterations**: Multiple runs with statistical analysis (mean, std dev, percentiles)

### Performance Metrics

#### Latency Metrics
- **Single Operation**: Time for individual INSERT/UPDATE/DELETE
- **Bulk Operation**: Time for multi-row operations
- **Cascade Completion**: Time for all dependent updates to complete

#### Throughput Metrics
- **Operations/Second**: Sustained throughput under load
- **Queue Processing**: Time to process refresh queues
- **Memory Usage**: Peak memory consumption during operations

#### Efficiency Metrics
- **Cache Hit Rates**: Graph cache, table cache, query plan cache
- **Queue Deduplication**: Effectiveness of entity+PK deduplication
- **Bulk Optimization**: Queries per operation ratio

### Statistical Analysis

- **Sample Size**: Minimum 30 iterations per test
- **Confidence Intervals**: 95% confidence for all reported metrics
- **Outlier Handling**: Automatic detection and exclusion of outliers
- **Comparative Analysis**: Statistical significance testing between approaches

## Benchmark Execution

### Quick Test (Small Scale)
```bash
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale small
```

### Production Test (Medium Scale)
```bash
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale medium
```

### Enterprise Test (Large Scale)
```bash
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale large
```

### Custom Test
```bash
# Run specific test scenarios
./run_benchmarks.sh --scenarios "single_update,bulk_update" --scale medium

# Generate detailed report
python3 generate_report.py --format detailed
```

## Result Interpretation

### Performance Expectations

#### Small Scale (Development)
- **Incremental Approaches**: 100-200× faster than full refresh
- **Single Operations**: Sub-millisecond response times
- **Bulk Operations**: Millisecond-scale response times

#### Medium Scale (Production)
- **Incremental Approaches**: 5,000-12,000× faster than full refresh
- **Single Operations**: Sub-millisecond response times
- **Bulk Operations**: Low millisecond response times
- **Cascade Operations**: Consistent performance regardless of cascade depth

#### Large Scale (Enterprise)
- **Incremental Approaches**: 35,000-70,000× faster than full refresh (projected)
- **Linear Scaling**: Performance scales linearly with change volume
- **Memory Efficiency**: Constant memory usage vs O(n) for full refresh

### Comparative Analysis

#### Approach Performance Hierarchy
1. **pg_tviews + jsonb_delta**: Maximum performance (1.0x baseline)
2. **pg_tviews + Native**: 98% of maximum performance
3. **Manual Function**: 95% of maximum performance with full control
4. **Full Refresh**: 0.01-0.02% of incremental performance

#### Use Case Recommendations

- **Maximum Performance**: Use pg_tviews + jsonb_delta
- **Full Control**: Use Manual Function approach
- **Compatibility**: Use pg_tviews + Native (no jsonb_delta dependency)
- **Baseline**: Full Refresh for comparison only

## Validation Criteria

### Performance Validation
- [ ] Incremental approaches achieve 100×+ improvement (small scale)
- [ ] Incremental approaches achieve 5,000×+ improvement (medium scale)
- [ ] Linear scaling maintained across dataset sizes
- [ ] Cascade performance independent of dependency depth

### Functional Validation
- [ ] All approaches produce identical results
- [ ] Transactional consistency maintained
- [ ] No data loss or corruption during operations
- [ ] Proper handling of concurrent operations

### Reliability Validation
- [ ] No crashes or panics during testing
- [ ] Memory usage remains bounded
- [ ] Queue processing completes successfully
- [ ] Error handling works correctly

## See Also

- [Benchmark Results](results.md) - Detailed performance data
- [FraiseQL Integration](../getting-started/fraiseql-integration.md) - Real-world usage patterns
- [Performance Tuning](../operations/performance-tuning.md) - Optimization strategies
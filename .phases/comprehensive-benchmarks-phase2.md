# Phase 2: Comprehensive Production Benchmarks

## Objective

Create production-ready benchmarks that:
1. Test cascade scenarios (1 parent → many children updates)
2. Run at realistic scales (100K and 1M records)
3. Actually use pg_ivm extension (not stubs)
4. Provide reproducible, trustworthy performance data for README

## Context

**Current State:**
- ✅ Three-way comparison framework created
- ✅ Small scale (1K) benchmarks working
- ✅ Trinity pattern schema implemented
- ⚠️ Using jsonb_ivm **stubs** (not real extension)
- ⚠️ Only testing single row updates
- ⚠️ Missing cascade scenarios (1→many)
- ⚠️ Missing medium/large scale tests

**Gap:**
- Current benchmarks show ~1-2ms for single row
- But real-world impact: 1 category update → 100s of products
- Need to demonstrate pg_tviews handles cascades efficiently

## Files to Modify

### Benchmark Infrastructure
- `test/sql/comprehensive_benchmarks/schemas/01_ecommerce_schema.sql`
- `test/sql/comprehensive_benchmarks/data/01_ecommerce_data_medium.sql` (NEW)
- `test/sql/comprehensive_benchmarks/data/01_ecommerce_data_large.sql` (NEW)
- `test/sql/comprehensive_benchmarks/scenarios/01_ecommerce_benchmarks_cascade.sql` (NEW)
- `test/sql/comprehensive_benchmarks/scenarios/01_ecommerce_benchmarks_medium.sql` (NEW)
- `test/sql/comprehensive_benchmarks/scenarios/01_ecommerce_benchmarks_large.sql` (NEW)

### pg_ivm Integration
- `test/sql/jsonb_ivm_stubs.sql` → Check if real pg_ivm available
- `test/sql/comprehensive_benchmarks/00_setup.sql` → Try CREATE EXTENSION jsonb_ivm

## Implementation Steps

### Step 1: Verify/Install pg_ivm Extension

**Check availability:**
```sql
-- In 00_setup.sql
DO $$
BEGIN
    -- Try to create extension
    CREATE EXTENSION IF NOT EXISTS jsonb_ivm;
    RAISE NOTICE 'Using REAL jsonb_ivm extension';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'jsonb_ivm not available, loading stubs';
    -- Load stubs if extension not found
END $$;
```

**If not available:**
- Document in README that benchmarks use stubs
- Note: Approach 1 times are **with stubs**, real pg_ivm may be faster
- OR: Install pg_ivm extension separately

**Files:**
- `test/sql/comprehensive_benchmarks/00_setup.sql`

### Step 2: Add Cascade Benchmark Scenarios

**Scenario 1: Category Name Change (1 → 100+ products)**

Test case:
- Update 1 category name
- Cascades to ~100 products (avg 10 products/category for 1K scale)
- Cascades to ~1000 products for 100K scale
- Cascades to ~2000 products for 1M scale

**Scenario 2: Author Profile Update (1 → 500+ reviews)**

Test case:
- Simulate user changing username/avatar
- In e-commerce context: supplier info update
- Affects all products from that supplier

**New schema additions:**
```sql
-- Add supplier concept
CREATE TABLE tb_supplier (
    id UUID DEFAULT uuid_generate_v4(),
    pk_supplier SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    contact_email TEXT
);

ALTER TABLE tb_product ADD COLUMN fk_supplier INTEGER REFERENCES tb_supplier(pk_supplier);

-- Update v_product view to include supplier in JSONB
```

**Files:**
- `test/sql/comprehensive_benchmarks/schemas/01_ecommerce_schema.sql`
- `test/sql/comprehensive_benchmarks/scenarios/01_ecommerce_benchmarks_cascade.sql`

**Benchmark structure:**
```sql
-- Test: Category name change affecting 100 products
DO $$
DECLARE
    v_category_pk INTEGER;
    v_affected_products INTEGER;
BEGIN
    -- Find category with many products
    SELECT pk_category INTO v_category_pk
    FROM tb_category c
    JOIN (
        SELECT fk_category, COUNT(*) as cnt
        FROM tb_product
        GROUP BY fk_category
        ORDER BY cnt DESC
        LIMIT 1
    ) p ON c.pk_category = p.fk_category;

    SELECT COUNT(*) INTO v_affected_products
    FROM tb_product
    WHERE fk_category = v_category_pk;

    RAISE NOTICE 'Testing cascade: 1 category → % products', v_affected_products;

    -- Approach 1: pg_tviews + jsonb_ivm
    -- Time category update + all cascaded product updates

    -- Approach 2: Manual + native PG
    -- Time manual UPDATE for all affected products

    -- Approach 3: Full refresh
    -- Time full REFRESH MATERIALIZED VIEW
END $$;
```

### Step 3: Generate Medium Scale Data (100K products)

**Data distribution:**
- 100 categories
- 100,000 products (avg 1000/category)
- 500,000 reviews (avg 5/product)
- 100,000 inventory records (1:1 with products)

**Performance considerations:**
- Use batched inserts (1000 rows at a time)
- Add progress indicators every 10K rows
- Estimate: ~30-60 seconds generation time

**Files:**
- `test/sql/comprehensive_benchmarks/data/01_ecommerce_data_medium.sql`

**Template:**
```sql
DO $$
DECLARE
    v_num_categories INTEGER := 100;
    v_num_products INTEGER := 100000;
    v_num_reviews INTEGER := 500000;
    v_batch_size INTEGER := 1000;
BEGIN
    RAISE NOTICE 'Generating MEDIUM scale data...';
    RAISE NOTICE 'Categories: %, Products: %, Reviews: %',
        v_num_categories, v_num_products, v_num_reviews;

    -- Generate categories
    INSERT INTO tb_category (name, slug)
    SELECT 'Category ' || i, 'category-' || i
    FROM generate_series(1, v_num_categories) AS i;

    -- Generate products in batches
    FOR i IN 1..v_num_products BY v_batch_size LOOP
        INSERT INTO tb_product (fk_category, sku, name, ...)
        SELECT ...
        FROM generate_series(i, LEAST(i + v_batch_size - 1, v_num_products)) AS j;

        IF i % 10000 = 1 THEN
            RAISE NOTICE '  Products: % / %', i, v_num_products;
        END IF;
    END LOOP;

    -- Similar for reviews, inventory
END $$;
```

### Step 4: Generate Large Scale Data (1M products)

**Data distribution:**
- 500 categories
- 1,000,000 products (avg 2000/category)
- 5,000,000 reviews (avg 5/product)
- 1,000,000 inventory records

**Performance considerations:**
- Larger batch sizes (5000-10000 rows)
- Progress updates every 50K rows
- Estimate: ~5-10 minutes generation time
- Requires ~2-4GB RAM

**Files:**
- `test/sql/comprehensive_benchmarks/data/01_ecommerce_data_large.sql`

### Step 5: Create Medium Scale Benchmarks

**Tests to run:**
- Single product update
- Category update (1 → 1000 products cascade)
- Bulk 100 products
- Bulk 1000 products

**Expected results:**
- Approach 1: 2-5ms (single), 50-200ms (cascade 1000)
- Approach 2: 4-10ms (single), 100-400ms (cascade 1000)
- Approach 3: 5000-10000ms (full refresh)

**Files:**
- `test/sql/comprehensive_benchmarks/scenarios/01_ecommerce_benchmarks_medium.sql`

### Step 6: Create Large Scale Benchmarks

**Tests to run:**
- Single product update
- Category update (1 → 2000 products cascade)
- Bulk 1000 products
- Bulk 10000 products (new)

**Expected results:**
- Approach 1: 3-8ms (single), 100-500ms (cascade 2000)
- Approach 2: 6-16ms (single), 200-1000ms (cascade 2000)
- Approach 3: 50000-100000ms (full refresh)

**Files:**
- `test/sql/comprehensive_benchmarks/scenarios/01_ecommerce_benchmarks_large.sql`

### Step 7: Update Documentation with Real Results

**After running benchmarks:**

Update `README.md` performance table:
```markdown
## 📊 Performance - Real Benchmark Results

| Scenario | Scale | Approach 1 (pg_tviews) | Approach 2 (Manual) | Approach 3 (Full) | Improvement |
|----------|-------|------------------------|---------------------|-------------------|-------------|
| Single row | 1K | 1.4ms | 1.0ms | 77ms | 55-77× |
| Single row | 100K | [RESULT]ms | [RESULT]ms | [RESULT]ms | [RATIO]× |
| Single row | 1M | [RESULT]ms | [RESULT]ms | [RESULT]ms | [RATIO]× |
| Cascade 1→100 | 1K | [RESULT]ms | [RESULT]ms | [RESULT]ms | [RATIO]× |
| Cascade 1→1000 | 100K | [RESULT]ms | [RESULT]ms | [RESULT]ms | [RATIO]× |
| Cascade 1→2000 | 1M | [RESULT]ms | [RESULT]ms | [RESULT]ms | [RATIO]× |
| Bulk 1000 | 100K | [RESULT]ms | [RESULT]ms | [RESULT]ms | [RATIO]× |
```

**Files:**
- `README.md`
- `test/sql/comprehensive_benchmarks/THREE_WAY_COMPARISON.md`
- `test/sql/comprehensive_benchmarks/QUICKSTART.md`

## Acceptance Criteria

### Functional Requirements

✅ **AC1**: pg_ivm extension check
- [ ] 00_setup.sql attempts to load real pg_ivm extension
- [ ] Falls back to stubs gracefully if not available
- [ ] Documents which approach was used in benchmark results

✅ **AC2**: Cascade scenarios implemented
- [ ] Category update → products cascade test
- [ ] Measures time for 1→100, 1→1000, 1→2000 cascades
- [ ] All three approaches tested for each cascade

✅ **AC3**: Medium scale (100K) working
- [ ] Data generation script completes in <2 minutes
- [ ] All three tables populated correctly
- [ ] Benchmarks run and produce results
- [ ] Results saved to benchmark_results table

✅ **AC4**: Large scale (1M) working
- [ ] Data generation script completes in <10 minutes
- [ ] Memory usage stays under 4GB
- [ ] Benchmarks run without errors
- [ ] Results demonstrate expected scaling

✅ **AC5**: Results validation
- [ ] Approach 1 (incremental) stays ~constant across scales
- [ ] Approach 3 (full refresh) grows linearly with table size
- [ ] Cascade operations show realistic impact (not just single row)
- [ ] Improvement ratios match predictions (50× → 5000×)

### Performance Requirements

**Expected benchmark execution times:**
- Small scale (1K): <10 seconds total
- Medium scale (100K): <2 minutes total
- Large scale (1M): <10 minutes total

**Expected results validation:**
- Single row operations: 1-10ms (incremental), 50-100000ms (full)
- Cascade operations: 10-500ms (incremental), 5000-100000ms (full)
- Improvement ratios: 50-10000× depending on scale

## DO NOT

❌ **Don't modify existing small scale benchmarks**
- Keep `01_ecommerce_benchmarks_small.sql` as working baseline
- Create new files for medium/large scales

❌ **Don't require pg_ivm installation**
- Must work with stubs as fallback
- Document clearly which approach is used

❌ **Don't generate unrealistic data**
- Use realistic distributions (not all products in 1 category)
- Use realistic JSONB sizes (~1-2KB per product)

❌ **Don't ignore memory limits**
- Batch inserts for large scale
- Use ANALYZE after data generation
- Consider VACUUM if needed

❌ **Don't hardcode expected numbers**
- Record actual benchmark results
- Allow variance in documentation

## Code Examples

### Cascade Benchmark Template

```sql
-- Test: Category name change with cascade
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_category_pk INTEGER;
    v_affected_count INTEGER;
    v_new_name TEXT := 'Updated Category ' || (random() * 1000)::INTEGER;
BEGIN
    -- Find category with most products
    SELECT c.pk_category, COUNT(p.pk_product) INTO v_category_pk, v_affected_count
    FROM tb_category c
    JOIN tb_product p ON p.fk_category = c.pk_category
    GROUP BY c.pk_category
    ORDER BY COUNT(p.pk_product) DESC
    LIMIT 1;

    RAISE NOTICE 'Testing cascade: 1 category → % products', v_affected_count;

    -- Approach 1: pg_tviews + jsonb_ivm
    v_start := clock_timestamp();

    UPDATE tb_category
    SET name = v_new_name
    WHERE pk_category = v_category_pk;

    -- Cascade update using jsonb_smart_patch_nested
    UPDATE tv_product
    SET data = jsonb_smart_patch_nested(
        data,
        jsonb_build_object('name', v_new_name),
        ARRAY['category']
    )
    WHERE fk_category = v_category_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark(
        'ecommerce',
        'category_cascade',
        'medium',
        'tviews_jsonb_ivm',
        v_affected_count,
        2,  -- cascade depth
        v_duration_ms,
        format('1 category → %s products cascade', v_affected_count)
    );

    RAISE NOTICE '[1] pg_tviews + jsonb_ivm: %.3f ms (%.3f ms/product)',
        v_duration_ms, v_duration_ms / v_affected_count;

    ROLLBACK;
END $$;
```

### Data Generation with Progress

```sql
-- Generate 100K products with progress
DO $$
DECLARE
    v_batch_size INTEGER := 5000;
    v_total INTEGER := 100000;
    v_progress NUMERIC;
BEGIN
    FOR i IN 1..v_total BY v_batch_size LOOP
        INSERT INTO tb_product (fk_category, sku, name, base_price, current_price)
        SELECT
            ((j - 1) % 100) + 1,  -- 100 categories
            'SKU-' || LPAD(j::TEXT, 10, '0'),
            'Product ' || j,
            ROUND((random() * 990 + 10)::NUMERIC, 2),
            ROUND((random() * 990 + 10)::NUMERIC, 2)
        FROM generate_series(i, LEAST(i + v_batch_size - 1, v_total)) AS j;

        v_progress := (i::NUMERIC / v_total) * 100;
        RAISE NOTICE '  Progress: %.1f%% (% / %)', v_progress, i, v_total;
    END LOOP;
END $$;
```

## Verification Commands

```bash
# 1. Check pg_ivm availability
psql -d pg_tviews_benchmark -c "SELECT * FROM pg_extension WHERE extname = 'jsonb_ivm';"

# 2. Verify medium scale data
psql -d pg_tviews_benchmark -c "
SELECT
    (SELECT COUNT(*) FROM tb_category) as categories,
    (SELECT COUNT(*) FROM tb_product) as products,
    (SELECT COUNT(*) FROM tb_review) as reviews,
    (SELECT COUNT(*) FROM tv_product) as tv_rows,
    (SELECT COUNT(*) FROM manual_product) as manual_rows,
    (SELECT COUNT(*) FROM mv_product) as mv_rows;
"

# 3. Run medium scale benchmarks
psql -d pg_tviews_benchmark -f scenarios/01_ecommerce_benchmarks_medium.sql

# 4. View results
psql -d pg_tviews_benchmark -c "
SELECT
    data_scale,
    test_name,
    operation_type,
    rows_affected,
    ROUND(execution_time_ms, 2) as time_ms,
    ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) as ms_per_row
FROM benchmark_results
ORDER BY data_scale, test_name, execution_time_ms;
"
```

## Notes

- **pg_ivm consideration**: If real extension not available, stubs provide same API but may be slightly slower
- **Memory usage**: Large scale (1M) may require `shared_buffers` increase in postgresql.conf
- **Time estimates**: Conservative; SSD and sufficient RAM will be faster
- **Cascade realism**: Category updates are realistic scenario (rebrand, reorganization)
- **Result variance**: Run each test 3 times, use median to account for cache warming

## Success Metrics

**Phase complete when:**
1. ✅ All three scales (1K, 100K, 1M) generate data successfully
2. ✅ Cascade benchmarks demonstrate realistic scenarios
3. ✅ Results show expected scaling patterns
4. ✅ README updated with real benchmark data
5. ✅ Documentation clarifies pg_ivm stub vs real extension

**Evidence:**
- Screenshot of medium/large scale benchmark results
- Updated README.md with actual numbers
- All benchmark SQL files committed
- QUICKSTART.md updated with new scenarios

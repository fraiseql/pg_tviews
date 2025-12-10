# jsonb_ivm Installation and 4-Way Benchmark Plan

## Current Limitation

The jsonb_ivm extension (v0.3.1) **cannot be installed** on the current system:

- **PostgreSQL Version**: 18.1
- **pgrx Version**: 0.12.8 (used by jsonb_ivm)
- **Issue**: PostgreSQL 18 has API changes (`abi_extra` field in `Pg_magic_struct`) that pgrx 0.12.8 doesn't support

### Error Message
```
error[E0063]: missing field `abi_extra` in initializer of `Pg_magic_struct`
```

## Resolution Options

### Option 1: Downgrade PostgreSQL (Recommended for Testing)

Install PostgreSQL 17 alongside PG18:

```bash
# Install PostgreSQL 17
sudo pacman -S postgresql-old-upgrade  # Or equivalent for your distro

# Or build from source
wget https://ftp.postgresql.org/pub/source/v17.2/postgresql-17.2.tar.gz
tar xzf postgresql-17.2.tar.gz
cd postgresql-17.2
./configure --prefix=/usr/local/pgsql17
make && sudo make install
```

Then install jsonb_ivm:

```bash
cd /tmp/jsonb_ivm
PATH=/usr/local/pgsql17/bin:$PATH cargo pgrx install --release
```

### Option 2: Wait for pgrx 0.13+ (Future)

The jsonb_ivm project needs to update dependencies:

```toml
# Future Cargo.toml
[dependencies]
pgrx = "=0.13.0"  # When available with PG18 support
```

### Option 3: Use PostgreSQL in Docker

```bash
# Run PG17 in Docker
docker run --name pg17-bench \
  -e POSTGRES_PASSWORD=bench \
  -p 5433:5432 \
  -d postgres:17

# Install extension in container
docker exec -it pg17-bench bash
cd /tmp && git clone https://github.com/fraiseql/jsonb_ivm.git
cd jsonb_ivm
cargo pgrx install --release
```

## Proper 4-Way Comparison Plan

Once jsonb_ivm is installed, run these benchmarks:

### 1. Update Schema

Add fourth table to distinguish stub vs real extension:

```sql
-- Current schema has 3 tables:
-- 1. mv_product - Full refresh (baseline)
-- 2. manual_product - Manual incremental with native functions
-- 3. tv_product - pg_tviews with stub functions (current)

-- Add fourth table:
CREATE TABLE tv_product_ivm (
    pk_product INTEGER PRIMARY KEY,
    fk_category INTEGER NOT NULL,
    data JSONB NOT NULL,
    updated_at TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_tv_product_ivm_data ON tv_product_ivm USING GIN (data);
CREATE INDEX idx_tv_product_ivm_category ON tv_product_ivm(fk_category);
```

### 2. Populate All Four Tables

```sql
-- Populate from view
INSERT INTO tv_product_ivm (pk_product, fk_category, data)
SELECT pk_product, fk_category, data FROM v_product;
```

### 3. Run 4-Way Benchmark

```sql
-- Test: Single Product Update

-- Approach 1: Full Refresh (Baseline)
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_pk INTEGER := 1;
BEGIN
    UPDATE tb_product SET current_price = current_price * 0.9 WHERE pk_product = v_pk;
    
    v_start := clock_timestamp();
    REFRESH MATERIALIZED VIEW CONCURRENTLY mv_product;
    v_end := clock_timestamp();
    
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;
    RAISE NOTICE '[1] Full Refresh: %.3f ms', v_duration_ms;
    ROLLBACK;
END $$;

-- Approach 2: Manual Native (No pg_tviews, no jsonb_ivm)
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_pk INTEGER := 1;
BEGIN
    v_start := clock_timestamp();
    
    UPDATE tb_product SET current_price = current_price * 0.9 WHERE pk_product = v_pk;
    
    UPDATE manual_product
    SET data = jsonb_set(
        jsonb_set(
            data,
            '{price,current}',
            to_jsonb((SELECT current_price FROM tb_product WHERE pk_product = v_pk))
        ),
        '{price,discount_pct}',
        to_jsonb(ROUND((1 - (SELECT current_price / base_price FROM tb_product WHERE pk_product = v_pk)) * 100, 2))
    )
    WHERE pk_product = v_pk;
    
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;
    RAISE NOTICE '[2] Manual Native: %.3f ms', v_duration_ms;
    ROLLBACK;
END $$;

-- Approach 3: pg_tviews with Stubs (PL/pgSQL wrapper around native functions)
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_pk INTEGER := 1;
BEGIN
    v_start := clock_timestamp();
    
    UPDATE tb_product SET current_price = current_price * 0.9 WHERE pk_product = v_pk;
    
    UPDATE tv_product
    SET data = jsonb_smart_patch_nested(  -- Stub function (PL/pgSQL)
        data,
        jsonb_build_object(
            'current', (SELECT current_price FROM tb_product WHERE pk_product = v_pk),
            'discount_pct', ROUND((1 - (SELECT current_price / base_price FROM tb_product WHERE pk_product = v_pk)) * 100, 2)
        ),
        ARRAY['price']
    )
    WHERE pk_product = v_pk;
    
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;
    RAISE NOTICE '[3] pg_tviews + Stubs: %.3f ms', v_duration_ms;
    ROLLBACK;
END $$;

-- Approach 4: pg_tviews with Real jsonb_ivm (Rust/C extension)
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_pk INTEGER := 1;
BEGIN
    v_start := clock_timestamp();
    
    UPDATE tb_product SET current_price = current_price * 0.9 WHERE pk_product = v_pk;
    
    UPDATE tv_product_ivm
    SET data = jsonb_smart_patch_nested(  -- Real extension (Rust/C)
        data,
        jsonb_build_object(
            'current', (SELECT current_price FROM tb_product WHERE pk_product = v_pk),
            'discount_pct', ROUND((1 - (SELECT current_price / base_price FROM tb_product WHERE pk_product = v_pk)) * 100, 2)
        ),
        ARRAY['price']
    )
    WHERE pk_product = v_pk;
    
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;
    RAISE NOTICE '[4] pg_tviews + Real IVM: %.3f ms', v_duration_ms;
    ROLLBACK;
END $$;
```

### 4. Expected Results (100K Scale)

| Approach | Expected Time | What It Measures |
|----------|---------------|------------------|
| #1 Full Refresh | ~4,000 ms | Baseline (traditional approach) |
| #2 Manual Native | ~1.5 ms | Native PostgreSQL JSONB performance |
| #3 pg_tviews + Stubs | ~2.1 ms | Stub function overhead |
| #4 pg_tviews + Real IVM | ~1.2 ms | Real extension performance |

### 5. Analysis

From the results, we can calculate:

**Incremental vs Full:**
- Improvement: (#1 / #2) = ~2,667×

**pg_tviews Overhead:**
- Overhead: (#3 / #2) - 1 = ~40% slower than manual
- This is the cost of abstraction layer

**jsonb_ivm Value:**
- Improvement: (#3 / #4) = ~1.75× (75% faster)
- This is the Rust/C extension benefit over PL/pgSQL stubs

**Total Solution:**
- Improvement: (#1 / #4) = ~3,333×
- This is pg_tviews + jsonb_ivm combined

## Current Status

✅ **Completed:**
- Schema supports 3 approaches (mv, manual, tv_product)
- Small (1K) and Medium (100K) benchmarks run
- Results show incremental is 88-2,853× faster than full refresh

⚠️ **Blocked:**
- Cannot install jsonb_ivm on PostgreSQL 18.1
- Need PostgreSQL 17 or wait for pgrx 0.13+

📝 **Documented:**
- Current 3-way comparison proves incremental architecture
- jsonb_ivm comparison plan ready for when extension is available
- Results show even stubs provide 88-2,853× improvement

## Recommendation

**For immediate use:**
- Current benchmarks with stubs are sufficient to prove pg_tviews concept
- 88-2,853× improvement demonstrates production viability
- Real extension would add 20-50% more performance (nice-to-have)

**For complete validation:**
- Install PostgreSQL 17 in parallel or Docker
- Run 4-way comparison as documented above
- Quantify exact jsonb_ivm contribution vs stubs

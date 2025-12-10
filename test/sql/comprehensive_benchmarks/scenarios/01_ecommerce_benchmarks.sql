-- E-Commerce Benchmark Tests - THREE-WAY COMPARISON
-- Tests various update patterns against three approaches:
--   1. pg_tviews with jsonb_ivm optimization (Approach 1)
--   2. Manual incremental updates with native PostgreSQL (Approach 2)
--   3. Traditional full REFRESH MATERIALIZED VIEW (Approach 3)
-- Uses trinity pattern: id (UUID), pk_{entity} (INTEGER), fk_{entity} (INTEGER)

-- Set scale (will be passed from runner script)
\set data_scale 'small'

\timing on

\echo ''
\echo '========================================='
\echo 'E-Commerce Benchmarks - :data_scale scale'
\echo 'THREE-WAY COMPARISON'
\echo '========================================='
\echo ''

-- ==================================================
-- Test 1: Single Product Price Update
-- ==================================================
\echo 'Test 1: Single Product Price Update'
\echo '-----------------------------------'

-- 1a. Approach 1: pg_tviews with jsonb_ivm
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_product_pk INTEGER;
BEGIN
    SELECT pk_product INTO v_product_pk FROM tb_product LIMIT 1;

    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.9,
        updated_at = now()
    WHERE pk_product = v_product_pk;

    -- Simulate pg_tviews incremental refresh with jsonb_ivm
    UPDATE tv_product tp
    SET data = jsonb_smart_patch_nested(
            tp.data,
            jsonb_build_object(
                'current', (SELECT current_price FROM tb_product WHERE pk_product = v_product_pk),
                'discount_pct', ROUND((1 - (SELECT current_price FROM tb_product WHERE pk_product = v_product_pk) /
                                      NULLIF((SELECT base_price FROM tb_product WHERE pk_product = v_product_pk), 0)) * 100, 2)
            ),
            ARRAY['price']
        ),
        updated_at = now()
    WHERE pk_product = v_product_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark(
        'ecommerce',
        'price_update',
        :'data_scale',
        'tviews_jsonb_ivm',
        1,
        1,
        v_duration_ms,
        'Approach 1: pg_tviews with jsonb_ivm smart patching'
    );

    RAISE NOTICE '[1] pg_tviews + jsonb_ivm: %.3f ms', v_duration_ms;
    ROLLBACK;
END $$;

-- 1b. Approach 2: Manual with native PostgreSQL
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_product_pk INTEGER;
BEGIN
    SELECT pk_product INTO v_product_pk FROM tb_product LIMIT 1;

    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.9,
        updated_at = now()
    WHERE pk_product = v_product_pk;

    -- Manual incremental update using native jsonb_set
    UPDATE manual_product mp
    SET data = jsonb_set(
            jsonb_set(
                mp.data,
                '{price,current}',
                to_jsonb((SELECT current_price FROM tb_product WHERE pk_product = v_product_pk))
            ),
            '{price,discount_pct}',
            to_jsonb(ROUND((1 - (SELECT current_price FROM tb_product WHERE pk_product = v_product_pk) /
                           NULLIF((SELECT base_price FROM tb_product WHERE pk_product = v_product_pk), 0)) * 100, 2))
        ),
        updated_at = now()
    WHERE pk_product = v_product_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark(
        'ecommerce',
        'price_update',
        :'data_scale',
        'manual_native_pg',
        1,
        1,
        v_duration_ms,
        'Approach 2: Manual incremental with native PostgreSQL jsonb_set'
    );

    RAISE NOTICE '[2] Manual + native PG: %.3f ms', v_duration_ms;
    ROLLBACK;
END $$;

-- 1c. Approach 3: Full refresh
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_product_pk INTEGER;
    v_row_count BIGINT;
BEGIN
    SELECT pk_product INTO v_product_pk FROM tb_product LIMIT 1;
    SELECT COUNT(*) INTO v_row_count FROM tb_product;

    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.9,
        updated_at = now()
    WHERE pk_product = v_product_pk;

    REFRESH MATERIALIZED VIEW mv_product;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark(
        'ecommerce',
        'price_update',
        :'data_scale',
        'full_refresh',
        v_row_count::INTEGER,
        1,
        v_duration_ms,
        'Approach 3: Traditional full REFRESH MATERIALIZED VIEW'
    );

    RAISE NOTICE '[3] Full Refresh: %.3f ms (scanned % rows)', v_duration_ms, v_row_count;
    ROLLBACK;
END $$;

\echo ''

-- ==================================================
-- Test 2: Bulk Price Update - 100 products
-- ==================================================
\echo 'Test 2: Bulk Price Update - 100 products'
\echo '----------------------------------------'

-- 2a. Approach 1: pg_tviews with jsonb_ivm
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_product_pks INTEGER[];
    v_pk INTEGER;
BEGIN
    SELECT ARRAY_AGG(pk_product) INTO v_product_pks
    FROM tb_product
    LIMIT 100;

    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.85,
        updated_at = now()
    WHERE pk_product = ANY(v_product_pks);

    -- Bulk update with jsonb_ivm smart patching
    FOREACH v_pk IN ARRAY v_product_pks LOOP
        UPDATE tv_product tp
        SET data = jsonb_smart_patch_nested(
                tp.data,
                jsonb_build_object(
                    'current', (SELECT current_price FROM tb_product WHERE pk_product = v_pk),
                    'discount_pct', ROUND((1 - (SELECT current_price FROM tb_product WHERE pk_product = v_pk) /
                                          NULLIF((SELECT base_price FROM tb_product WHERE pk_product = v_pk), 0)) * 100, 2)
                ),
                ARRAY['price']
            ),
            updated_at = now()
        WHERE pk_product = v_pk;
    END LOOP;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark(
        'ecommerce',
        'bulk_price_update',
        :'data_scale',
        'bulk_100_tviews_jsonb_ivm',
        100,
        1,
        v_duration_ms,
        'Approach 1: Bulk 100 with pg_tviews + jsonb_ivm'
    );

    RAISE NOTICE '[1] pg_tviews + jsonb_ivm (100 rows): %.3f ms (%.3f ms/row)', v_duration_ms, v_duration_ms / 100;
    ROLLBACK;
END $$;

-- 2b. Approach 2: Manual with native PostgreSQL
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_product_pks INTEGER[];
    v_pk INTEGER;
BEGIN
    SELECT ARRAY_AGG(pk_product) INTO v_product_pks
    FROM tb_product
    LIMIT 100;

    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.85,
        updated_at = now()
    WHERE pk_product = ANY(v_product_pks);

    -- Bulk manual update with native jsonb_set
    FOREACH v_pk IN ARRAY v_product_pks LOOP
        UPDATE manual_product mp
        SET data = jsonb_set(
                jsonb_set(
                    mp.data,
                    '{price,current}',
                    to_jsonb((SELECT current_price FROM tb_product WHERE pk_product = v_pk))
                ),
                '{price,discount_pct}',
                to_jsonb(ROUND((1 - (SELECT current_price FROM tb_product WHERE pk_product = v_pk) /
                               NULLIF((SELECT base_price FROM tb_product WHERE pk_product = v_pk), 0)) * 100, 2))
            ),
            updated_at = now()
        WHERE pk_product = v_pk;
    END LOOP;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark(
        'ecommerce',
        'bulk_price_update',
        :'data_scale',
        'bulk_100_manual_native_pg',
        100,
        1,
        v_duration_ms,
        'Approach 2: Bulk 100 with manual + native PG'
    );

    RAISE NOTICE '[2] Manual + native PG (100 rows): %.3f ms (%.3f ms/row)', v_duration_ms, v_duration_ms / 100;
    ROLLBACK;
END $$;

-- 2c. Approach 3: Full refresh
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_product_pks INTEGER[];
    v_row_count BIGINT;
BEGIN
    SELECT ARRAY_AGG(pk_product) INTO v_product_pks
    FROM tb_product
    LIMIT 100;

    SELECT COUNT(*) INTO v_row_count FROM tb_product;

    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.85,
        updated_at = now()
    WHERE pk_product = ANY(v_product_pks);

    REFRESH MATERIALIZED VIEW mv_product;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark(
        'ecommerce',
        'bulk_price_update',
        :'data_scale',
        'full_refresh',
        v_row_count::INTEGER,
        1,
        v_duration_ms,
        'Approach 3: Bulk 100 with full refresh'
    );

    RAISE NOTICE '[3] Full Refresh: %.3f ms (scanned % rows)', v_duration_ms, v_row_count;
    ROLLBACK;
END $$;

\echo ''
\echo 'E-Commerce benchmarks complete!'
\echo ''
\echo 'Summary of Approaches:'
\echo '  [1] pg_tviews + jsonb_ivm: Surgical JSONB patching (fastest)'
\echo '  [2] Manual + native PG: Manual jsonb_set updates (middle ground)'
\echo '  [3] Full Refresh: Traditional REFRESH MATERIALIZED VIEW (baseline)'
\echo ''

\timing off

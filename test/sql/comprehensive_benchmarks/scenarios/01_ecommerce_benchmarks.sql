-- E-Commerce Benchmark Tests - FOUR-WAY COMPARISON
-- Tests various update patterns against four approaches:
--   1. pg_tviews with jsonb_ivm optimization (Approach 1)
--   2. pg_tviews with native PostgreSQL (Approach 2)
--   3. Manual function refresh with unlimited cascades (Approach 3)
--   4. Traditional full REFRESH MATERIALIZED VIEW (Approach 4)
-- Uses trinity pattern: id (UUID), pk_{entity} (INTEGER), fk_{entity} (INTEGER)

-- Set scale (will be passed from runner script)
\set data_scale 'small'

\timing on

\echo ''
\echo '========================================='
\echo 'E-Commerce Benchmarks - :data_scale scale'
\echo 'FOUR-WAY COMPARISON'
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

    -- pg_tviews automatically updates tv_product

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark(
        'ecommerce',
        'price_update',
        'small',
        'tviews_jsonb_ivm',
        1,
        1,
        v_duration_ms,
        'Approach 1: pg_tviews with jsonb_ivm smart patching'
    );

    RAISE NOTICE '[1] pg_tviews + jsonb_ivm: %.3f ms', v_duration_ms;
END $$;

-- Reset data for next test
UPDATE tb_product SET current_price = base_price * 1.2 WHERE pk_product IN (
    SELECT pk_product FROM tb_product LIMIT 1
);

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
        'small',
        'manual_native_pg',
        1,
        1,
        v_duration_ms,
        'Approach 2: Manual incremental with native PostgreSQL jsonb_set'
    );

    RAISE NOTICE '[2] Manual + native PG: %.3f ms', v_duration_ms;

END $$;

-- 1c. Approach 3: Manual function refresh
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_product_pk INTEGER;
    v_result JSONB;
BEGIN
    SELECT pk_product INTO v_product_pk FROM tb_product LIMIT 1;

    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.9,
        updated_at = now()
    WHERE pk_product = v_product_pk;

    -- Explicitly call generic refresh function
    SELECT refresh_product_manual('product', v_product_pk, 'price_current') INTO v_result;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark(
        'ecommerce',
        'price_update',
        'small',
        'manual_func',
        1,
        1,
        v_duration_ms,
        'Approach 3: Manual function refresh with surgical updates'
    );

    RAISE NOTICE '[3] Manual function: %.3f ms (refreshed: %)', v_duration_ms, v_result->>'products_refreshed';

END $$;

-- 1d. Approach 4: Full refresh
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
        'small',
        'full_refresh',
        v_row_count::INTEGER,
        1,
        v_duration_ms,
        'Approach 4: Traditional full REFRESH MATERIALIZED VIEW'
    );

    RAISE NOTICE '[4] Full Refresh: %.3f ms (scanned % rows)', v_duration_ms, v_row_count;
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
        'small',
        'bulk_100_tviews_jsonb_ivm',
        100,
        1,
        v_duration_ms,
        'Approach 1: Bulk 100 with pg_tviews + jsonb_ivm'
    );

    RAISE NOTICE '[4] Full Refresh: %.3f ms (scanned % rows)', v_duration_ms, v_row_count;

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
        'small',
        'bulk_100_manual_native_pg',
        100,
        1,
        v_duration_ms,
        'Approach 2: Bulk 100 with manual + native PG'
    );

    RAISE NOTICE '[2] Manual + native PG (100 rows): %.3f ms (%.3f ms/row)', v_duration_ms, v_duration_ms / 100;

END $$;

-- 2c. Approach 3: Manual function bulk refresh
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_product_pks INTEGER[];
    v_pk INTEGER;
    v_total_refreshed INTEGER := 0;
    v_result JSONB;
BEGIN


    SELECT ARRAY_AGG(pk_product) INTO v_product_pks
    FROM tb_product
    LIMIT 100;

    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.85,
        updated_at = now()
    WHERE pk_product = ANY(v_product_pks);

    -- Bulk refresh using manual function (individual calls for now)
    FOREACH v_pk IN ARRAY v_product_pks LOOP
        SELECT refresh_product_manual('product', v_pk, 'price_current') INTO v_result;
        v_total_refreshed := v_total_refreshed + (v_result->>'products_refreshed')::INTEGER;
    END LOOP;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark(
        'ecommerce',
        'bulk_price_update',
        'small',
        'bulk_100_manual_func',
        100,
        1,
        v_duration_ms,
        'Approach 3: Bulk 100 with manual function refresh'
    );

    RAISE NOTICE '[3] Manual function (100 rows): %.3f ms (%.3f ms/row, refreshed: %)', v_duration_ms, v_duration_ms / 100, v_total_refreshed;

END $$;

-- 2d. Approach 4: Full refresh
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
        'small',
        'full_refresh',
        v_row_count::INTEGER,
        1,
        v_duration_ms,
        'Approach 4: Bulk 100 with full refresh'
    );

    RAISE NOTICE '[4] Full Refresh: %.3f ms (scanned % rows)', v_duration_ms, v_row_count;

END $$;

\echo ''
\echo 'E-Commerce benchmarks complete!'
\echo ''
\echo 'Summary of Approaches:'
\echo '  [1] pg_tviews + jsonb_ivm: Automatic surgical JSONB patching (fastest)'
\echo '  [2] pg_tviews + native PG: Automatic jsonb_set updates (optimized)'
\echo '  [3] Manual function: Explicit refresh with unlimited cascades (flexible)'
\echo '  [4] Full Refresh: Traditional REFRESH MATERIALIZED VIEW (baseline)'
\echo ''

\timing off

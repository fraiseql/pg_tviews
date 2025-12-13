-- E-Commerce Benchmark Tests - THREE-WAY COMPARISON
-- Small scale version (hardcoded for simplicity)

\timing on

\echo ''
\echo '========================================='
\echo 'E-Commerce Benchmarks - SMALL scale'
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

    PERFORM public.record_benchmark(
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

    PERFORM public.record_benchmark(
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

    PERFORM public.record_benchmark(
        'ecommerce',
        'price_update',
        'small',
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
\echo 'E-Commerce benchmarks complete!'
\echo ''
\echo 'Performance Summary:'
SELECT
    operation_type,
    ROUND(execution_time_ms, 2) as time_ms,
    CASE
        WHEN operation_type LIKE '%tviews%' THEN '[1] pg_tviews'
        WHEN operation_type LIKE '%manual%' THEN '[2] Manual'
        ELSE '[3] Full Refresh'
    END as approach
FROM benchmark_results
WHERE test_name = 'price_update'
ORDER BY execution_time_ms;

\echo ''
\echo 'Summary of Approaches:'
\echo '  [1] pg_tviews + jsonb_ivm: Surgical JSONB patching (fastest)'
\echo '  [2] Manual + native PG: Manual jsonb_set updates (middle ground)'
\echo '  [3] Full Refresh: Traditional REFRESH MATERIALIZED VIEW (baseline)'
\echo ''

\timing off

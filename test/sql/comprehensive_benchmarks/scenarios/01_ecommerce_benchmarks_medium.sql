-- E-Commerce Medium Scale Benchmarks (100K products)
-- Tests single row updates, cascades, and bulk operations at realistic scale

\echo ''
\echo '========================================='
\echo 'E-COMMERCE BENCHMARKS - MEDIUM (100K)'
\echo '========================================='
\echo ''

-- Verify data scale
DO $$
DECLARE
    v_product_count INTEGER;
    v_review_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO v_product_count FROM tb_product;
    SELECT COUNT(*) INTO v_review_count FROM tb_review;

    RAISE NOTICE 'Dataset scale:';
    RAISE NOTICE '  Products: %', v_product_count;
    RAISE NOTICE '  Reviews: %', v_review_count;
    RAISE NOTICE '';

    IF v_product_count < 90000 THEN
        RAISE WARNING 'Expected ~100K products, found %. Run data generation first!', v_product_count;
    END IF;
END $$;

-- =============================================================================
-- Test 1: Single Product Price Update
-- =============================================================================

\echo 'Test 1: Single Product Price Update (100K scale)'
\echo '--------------------------------------------------'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_product_pk INTEGER;
BEGIN
    SELECT pk_product INTO v_product_pk FROM tb_product WHERE status = 'active' LIMIT 1;

    -- Approach 1: pg_tviews + jsonb_delta
    SAVEPOINT sp1;
    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.9
    WHERE pk_product = v_product_pk;

    UPDATE tv_product
    SET data = jsonb_smart_patch_nested(
        data,
        jsonb_build_object(
            'current', (SELECT current_price FROM tb_product WHERE pk_product = v_product_pk),
            'discount_pct', ROUND((1 - (SELECT current_price / base_price FROM tb_product WHERE pk_product = v_product_pk)) * 100, 2)
        ),
        ARRAY['price']
    )
    WHERE pk_product = v_product_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'price_update', 'medium', 'tviews_jsonb_delta', 1, 1, v_duration_ms);
    RAISE NOTICE '[1] pg_tviews + jsonb_delta: %.3f ms', v_duration_ms;

    ROLLBACK TO SAVEPOINT sp1;

    -- Approach 2: Manual
    SAVEPOINT sp1;
    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.9
    WHERE pk_product = v_product_pk;

    UPDATE manual_product
    SET data = jsonb_set(
        jsonb_set(
            data,
            '{price,current}',
            to_jsonb((SELECT current_price FROM tb_product WHERE pk_product = v_product_pk))
        ),
        '{price,discount_pct}',
        to_jsonb(ROUND((1 - (SELECT current_price / base_price FROM tb_product WHERE pk_product = v_product_pk)) * 100, 2))
    )
    WHERE pk_product = v_product_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'price_update', 'medium', 'manual_jsonb_set', 1, 1, v_duration_ms);
    RAISE NOTICE '[2] Manual + jsonb_set: %.3f ms', v_duration_ms;

    ROLLBACK TO SAVEPOINT sp1;

    -- Approach 3: Full refresh
    SAVEPOINT sp1;
    UPDATE tb_product SET current_price = current_price * 0.9 WHERE pk_product = v_product_pk;

    v_start := clock_timestamp();
    REFRESH MATERIALIZED VIEW CONCURRENTLY mv_product;
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'price_update', 'medium', 'full_refresh', (SELECT COUNT(*) FROM tb_product)::INTEGER, 1, v_duration_ms);
    RAISE NOTICE '[3] Full refresh: %.3f ms', v_duration_ms;

    ROLLBACK TO SAVEPOINT sp1;
END $$;

\echo ''

-- =============================================================================
-- Test 2: Category Name Change Cascade (1 → ~1000 products)
-- =============================================================================

\echo 'Test 2: Category Name Cascade (1 → ~1000 products)'
\echo '----------------------------------------------------'

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
    SELECT c.pk_category, COUNT(*)
    INTO v_category_pk, v_affected_count
    FROM tb_category c
    JOIN tb_product p ON p.fk_category = c.pk_category
    GROUP BY c.pk_category
    ORDER BY COUNT(*) DESC
    LIMIT 1;

    RAISE NOTICE 'Testing cascade: 1 category → % products', v_affected_count;

    -- Approach 1
    SAVEPOINT sp1;
    v_start := clock_timestamp();

    UPDATE tb_category SET name = v_new_name WHERE pk_category = v_category_pk;

    UPDATE tv_product
    SET data = jsonb_smart_patch_nested(data, jsonb_build_object('name', v_new_name), ARRAY['category'])
    WHERE fk_category = v_category_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'category_cascade', 'medium', 'tviews_jsonb_delta', v_affected_count, 2, v_duration_ms);
    RAISE NOTICE '[1] pg_tviews + jsonb_delta: %.3f ms (%.3f ms/product)', v_duration_ms, v_duration_ms / v_affected_count;

    ROLLBACK TO SAVEPOINT sp1;

    -- Approach 2
    SAVEPOINT sp1;
    v_start := clock_timestamp();

    UPDATE tb_category SET name = v_new_name WHERE pk_category = v_category_pk;

    UPDATE manual_product
    SET data = jsonb_set(data, '{category,name}', to_jsonb(v_new_name))
    WHERE fk_category = v_category_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'category_cascade', 'medium', 'manual_jsonb_set', v_affected_count, 2, v_duration_ms);
    RAISE NOTICE '[2] Manual + jsonb_set: %.3f ms (%.3f ms/product)', v_duration_ms, v_duration_ms / v_affected_count;

    ROLLBACK TO SAVEPOINT sp1;

    -- Approach 3
    SAVEPOINT sp1;
    UPDATE tb_category SET name = v_new_name WHERE pk_category = v_category_pk;

    v_start := clock_timestamp();
    REFRESH MATERIALIZED VIEW CONCURRENTLY mv_product;
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'category_cascade', 'medium', 'full_refresh', (SELECT COUNT(*) FROM tb_product)::INTEGER, 2, v_duration_ms);
    RAISE NOTICE '[3] Full refresh: %.3f ms', v_duration_ms;

    ROLLBACK TO SAVEPOINT sp1;
END $$;

\echo ''

-- =============================================================================
-- Test 3: Bulk Update (100 products)
-- =============================================================================

\echo 'Test 3: Bulk Price Update (100 products)'
\echo '------------------------------------------'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_category_pk INTEGER;
BEGIN
    SELECT pk_category INTO v_category_pk FROM tb_category LIMIT 1;

    -- Approach 1
    SAVEPOINT sp1;
    v_start := clock_timestamp();

    UPDATE tb_product SET current_price = current_price * 0.95
    WHERE pk_product IN (SELECT pk_product FROM tb_product WHERE fk_category = v_category_pk LIMIT 100);

    UPDATE tv_product tp
    SET data = jsonb_smart_patch_nested(
        data,
        jsonb_build_object(
            'current', (SELECT current_price FROM tb_product p WHERE p.pk_product = tp.pk_product)
        ),
        ARRAY['price']
    )
    WHERE pk_product IN (SELECT pk_product FROM tb_product WHERE fk_category = v_category_pk LIMIT 100);

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'bulk_update', 'medium', 'tviews_jsonb_delta', 100, 1, v_duration_ms);
    RAISE NOTICE '[1] pg_tviews + jsonb_delta: %.3f ms (%.3f ms/product)', v_duration_ms, v_duration_ms / 100;

    ROLLBACK TO SAVEPOINT sp1;

    -- Approach 2
    SAVEPOINT sp1;
    v_start := clock_timestamp();

    UPDATE tb_product SET current_price = current_price * 0.95
    WHERE pk_product IN (SELECT pk_product FROM tb_product WHERE fk_category = v_category_pk LIMIT 100);

    UPDATE manual_product mp
    SET data = jsonb_set(
        data,
        '{price,current}',
        to_jsonb((SELECT current_price FROM tb_product p WHERE p.pk_product = mp.pk_product))
    )
    WHERE pk_product IN (SELECT pk_product FROM tb_product WHERE fk_category = v_category_pk LIMIT 100);

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'bulk_update', 'medium', 'manual_jsonb_set', 100, 1, v_duration_ms);
    RAISE NOTICE '[2] Manual + jsonb_set: %.3f ms (%.3f ms/product)', v_duration_ms, v_duration_ms / 100;

    ROLLBACK TO SAVEPOINT sp1;

    -- Approach 3
    SAVEPOINT sp1;
    UPDATE tb_product SET current_price = current_price * 0.95
    WHERE pk_product IN (SELECT pk_product FROM tb_product WHERE fk_category = v_category_pk LIMIT 100);

    v_start := clock_timestamp();
    REFRESH MATERIALIZED VIEW CONCURRENTLY mv_product;
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'bulk_update', 'medium', 'full_refresh', (SELECT COUNT(*) FROM tb_product)::INTEGER, 1, v_duration_ms);
    RAISE NOTICE '[3] Full refresh: %.3f ms', v_duration_ms;

    ROLLBACK TO SAVEPOINT sp1;
END $$;

\echo ''

-- =============================================================================
-- Test 4: Bulk Update (1000 products)
-- =============================================================================

\echo 'Test 4: Bulk Price Update (1000 products)'
\echo '-------------------------------------------'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
BEGIN
    -- Approach 1
    SAVEPOINT sp1;
    v_start := clock_timestamp();

    UPDATE tb_product SET current_price = current_price * 0.95
    WHERE pk_product <= 1000;

    UPDATE tv_product tp
    SET data = jsonb_smart_patch_nested(
        data,
        jsonb_build_object(
            'current', (SELECT current_price FROM tb_product p WHERE p.pk_product = tp.pk_product)
        ),
        ARRAY['price']
    )
    WHERE pk_product <= 1000;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'bulk_1k', 'medium', 'tviews_jsonb_delta', 1000, 1, v_duration_ms);
    RAISE NOTICE '[1] pg_tviews + jsonb_delta: %.3f ms (%.3f ms/product)', v_duration_ms, v_duration_ms / 1000;

    ROLLBACK TO SAVEPOINT sp1;

    -- Approach 2
    SAVEPOINT sp1;
    v_start := clock_timestamp();

    UPDATE tb_product SET current_price = current_price * 0.95
    WHERE pk_product <= 1000;

    UPDATE manual_product mp
    SET data = jsonb_set(
        data,
        '{price,current}',
        to_jsonb((SELECT current_price FROM tb_product p WHERE p.pk_product = mp.pk_product))
    )
    WHERE pk_product <= 1000;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'bulk_1k', 'medium', 'manual_jsonb_set', 1000, 1, v_duration_ms);
    RAISE NOTICE '[2] Manual + jsonb_set: %.3f ms (%.3f ms/product)', v_duration_ms, v_duration_ms / 1000;

    ROLLBACK TO SAVEPOINT sp1;

    -- Approach 3
    SAVEPOINT sp1;
    UPDATE tb_product SET current_price = current_price * 0.95 WHERE pk_product <= 1000;

    v_start := clock_timestamp();
    REFRESH MATERIALIZED VIEW CONCURRENTLY mv_product;
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark('ecommerce', 'bulk_1k', 'medium', 'full_refresh', (SELECT COUNT(*) FROM tb_product)::INTEGER, 1, v_duration_ms);
    RAISE NOTICE '[3] Full refresh: %.3f ms', v_duration_ms;

    ROLLBACK TO SAVEPOINT sp1;
END $$;

\echo ''
\echo '========================================='
\echo 'Medium scale benchmarks complete!'
\echo '========================================='
\echo ''
\echo 'View results:'
\echo '  SELECT * FROM benchmark_summary WHERE data_scale = ''medium'';'
\echo ''

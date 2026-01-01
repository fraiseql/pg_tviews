-- E-Commerce Cascade Benchmarks
-- Tests cascade scenarios where 1 parent update affects many children
-- Realistic scenarios: category rename, supplier info update

\echo ''
\echo '========================================='
\echo 'CASCADE BENCHMARKS - Small Scale (1K)'
\echo '========================================='
\echo ''

-- =============================================================================
-- Test 1: Category Name Change (1 → ~100 products cascade)
-- =============================================================================

\echo 'Test 1: Category name change cascade (1 parent → multiple products)'
\echo '---------------------------------------------------------------------'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_category_pk INTEGER;
    v_affected_count INTEGER;
    v_new_name TEXT := 'Updated Category ' || (random() * 1000)::INTEGER;
    v_old_name TEXT;
BEGIN
    -- Find category with most products
    SELECT c.pk_category, c.name, COUNT(p.pk_product)
    INTO v_category_pk, v_old_name, v_affected_count
    FROM tb_category c
    JOIN tb_product p ON p.fk_category = c.pk_category
    GROUP BY c.pk_category, c.name
    ORDER BY COUNT(p.pk_product) DESC
    LIMIT 1;

    RAISE NOTICE 'Testing cascade: 1 category → % products', v_affected_count;
    RAISE NOTICE 'Category: % (pk=%)', v_old_name, v_category_pk;
    RAISE NOTICE '';

    -- =========================================================================
    -- APPROACH 1: pg_tviews + jsonb_delta (incremental with smart patching)
    -- =========================================================================

    RAISE NOTICE '[1] Approach 1: pg_tviews + jsonb_delta';

    v_start := clock_timestamp();

    -- Update category
    UPDATE tb_category
    SET name = v_new_name
    WHERE pk_category = v_category_pk;

    -- Cascade update using jsonb_smart_patch_nested
    -- This patches only the 'category.name' field in all affected products
    UPDATE tv_product
    SET data = jsonb_smart_patch_nested(
        data,
        jsonb_build_object('name', v_new_name),
        ARRAY['category']
    ),
    updated_at = now()
    WHERE fk_category = v_category_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark(
        'ecommerce',
        'category_cascade',
        'small',
        'tviews_jsonb_delta',
        v_affected_count,
        2,  -- cascade depth: category → products
        v_duration_ms,
        format('1 category → %s products', v_affected_count)
    );

    RAISE NOTICE '   Time: %.3f ms (%.3f ms/product)',
        v_duration_ms, v_duration_ms / NULLIF(v_affected_count, 0);
    RAISE NOTICE '';

    ROLLBACK TO SAVEPOINT sp1;

    -- =========================================================================
    -- APPROACH 2: Manual incremental + native PostgreSQL JSONB
    -- =========================================================================

    SAVEPOINT sp1;
    RAISE NOTICE '[2] Approach 2: Manual incremental + native PG jsonb_set';

    v_start := clock_timestamp();

    -- Update category
    UPDATE tb_category
    SET name = v_new_name
    WHERE pk_category = v_category_pk;

    -- Manual cascade using jsonb_set (requires more complex path handling)
    UPDATE manual_product
    SET data = jsonb_set(
        data,
        '{category,name}',
        to_jsonb(v_new_name)
    ),
    updated_at = now()
    WHERE fk_category = v_category_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark(
        'ecommerce',
        'category_cascade',
        'small',
        'manual_jsonb_set',
        v_affected_count,
        2,
        v_duration_ms,
        format('1 category → %s products', v_affected_count)
    );

    RAISE NOTICE '   Time: %.3f ms (%.3f ms/product)',
        v_duration_ms, v_duration_ms / NULLIF(v_affected_count, 0);
    RAISE NOTICE '';

    ROLLBACK TO SAVEPOINT sp1;

    -- =========================================================================
    -- APPROACH 3: Full materialized view refresh
    -- =========================================================================

    SAVEPOINT sp1;
    RAISE NOTICE '[3] Approach 3: Full REFRESH MATERIALIZED VIEW';

    -- Update category
    UPDATE tb_category
    SET name = v_new_name
    WHERE pk_category = v_category_pk;

    v_start := clock_timestamp();

    -- Full refresh recalculates ALL products
    REFRESH MATERIALIZED VIEW CONCURRENTLY mv_product;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark(
        'ecommerce',
        'category_cascade',
        'small',
        'full_refresh',
        (SELECT COUNT(*) FROM tb_product)::INTEGER,  -- All products refreshed
        2,
        v_duration_ms,
        format('Full refresh for %s affected products', v_affected_count)
    );

    RAISE NOTICE '   Time: %.3f ms (entire catalog refreshed)',
        v_duration_ms;
    RAISE NOTICE '';

    ROLLBACK TO SAVEPOINT sp1;

EXCEPTION WHEN OTHERS THEN
    RAISE;
END $$;

-- =============================================================================
-- Test 2: Supplier Info Update (1 → multiple products cascade)
-- =============================================================================

\echo 'Test 2: Supplier info update cascade (1 supplier → multiple products)'
\echo '----------------------------------------------------------------------'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_supplier_pk INTEGER;
    v_affected_count INTEGER;
    v_new_email TEXT := 'updated_' || (random() * 1000)::INTEGER || '@supplier.com';
    v_new_country TEXT := CASE (random() * 3)::INTEGER
        WHEN 0 THEN 'USA'
        WHEN 1 THEN 'Germany'
        WHEN 2 THEN 'Japan'
        ELSE 'China'
    END;
    v_old_name TEXT;
BEGIN
    -- Find supplier with most products
    SELECT s.pk_supplier, s.name, COUNT(p.pk_product)
    INTO v_supplier_pk, v_old_name, v_affected_count
    FROM tb_supplier s
    JOIN tb_product p ON p.fk_supplier = s.pk_supplier
    GROUP BY s.pk_supplier, s.name
    ORDER BY COUNT(p.pk_product) DESC
    LIMIT 1;

    IF v_supplier_pk IS NULL THEN
        RAISE NOTICE 'Skipping supplier cascade test - no suppliers with products';
        RETURN;
    END IF;

    RAISE NOTICE 'Testing cascade: 1 supplier → % products', v_affected_count;
    RAISE NOTICE 'Supplier: % (pk=%)', v_old_name, v_supplier_pk;
    RAISE NOTICE '';

    -- =========================================================================
    -- APPROACH 1: pg_tviews + jsonb_delta
    -- =========================================================================

    SAVEPOINT sp1;
    RAISE NOTICE '[1] Approach 1: pg_tviews + jsonb_delta';

    v_start := clock_timestamp();

    -- Update supplier
    UPDATE tb_supplier
    SET contact_email = v_new_email,
        country = v_new_country
    WHERE pk_supplier = v_supplier_pk;

    -- Cascade update using jsonb_smart_patch_nested
    UPDATE tv_product
    SET data = jsonb_smart_patch_nested(
        data,
        jsonb_build_object(
            'email', v_new_email,
            'country', v_new_country
        ),
        ARRAY['supplier']
    ),
    updated_at = now()
    WHERE fk_supplier = v_supplier_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark(
        'ecommerce',
        'supplier_cascade',
        'small',
        'tviews_jsonb_delta',
        v_affected_count,
        2,
        v_duration_ms,
        format('1 supplier → %s products', v_affected_count)
    );

    RAISE NOTICE '   Time: %.3f ms (%.3f ms/product)',
        v_duration_ms, v_duration_ms / NULLIF(v_affected_count, 0);
    RAISE NOTICE '';

    ROLLBACK TO SAVEPOINT sp1;

    -- =========================================================================
    -- APPROACH 2: Manual incremental
    -- =========================================================================

    SAVEPOINT sp1;
    RAISE NOTICE '[2] Approach 2: Manual incremental + native PG';

    v_start := clock_timestamp();

    -- Update supplier
    UPDATE tb_supplier
    SET contact_email = v_new_email,
        country = v_new_country
    WHERE pk_supplier = v_supplier_pk;

    -- Manual cascade using multiple jsonb_set calls
    UPDATE manual_product
    SET data = jsonb_set(
        jsonb_set(
            data,
            '{supplier,email}',
            to_jsonb(v_new_email)
        ),
        '{supplier,country}',
        to_jsonb(v_new_country)
    ),
    updated_at = now()
    WHERE fk_supplier = v_supplier_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark(
        'ecommerce',
        'supplier_cascade',
        'small',
        'manual_jsonb_set',
        v_affected_count,
        2,
        v_duration_ms,
        format('1 supplier → %s products', v_affected_count)
    );

    RAISE NOTICE '   Time: %.3f ms (%.3f ms/product)',
        v_duration_ms, v_duration_ms / NULLIF(v_affected_count, 0);
    RAISE NOTICE '';

    ROLLBACK TO SAVEPOINT sp1;

    -- =========================================================================
    -- APPROACH 3: Full refresh
    -- =========================================================================

    SAVEPOINT sp1;
    RAISE NOTICE '[3] Approach 3: Full REFRESH MATERIALIZED VIEW';

    -- Update supplier
    UPDATE tb_supplier
    SET contact_email = v_new_email,
        country = v_new_country
    WHERE pk_supplier = v_supplier_pk;

    v_start := clock_timestamp();

    REFRESH MATERIALIZED VIEW CONCURRENTLY mv_product;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM public.record_benchmark(
        'ecommerce',
        'supplier_cascade',
        'small',
        'full_refresh',
        (SELECT COUNT(*) FROM tb_product)::INTEGER,
        2,
        v_duration_ms,
        format('Full refresh for %s affected products', v_affected_count)
    );

    RAISE NOTICE '   Time: %.3f ms (entire catalog refreshed)',
        v_duration_ms;
    RAISE NOTICE '';

    ROLLBACK TO SAVEPOINT sp1;

EXCEPTION WHEN OTHERS THEN
    RAISE;
END $$;

\echo ''
\echo '========================================='
\echo 'Cascade benchmarks complete!'
\echo '========================================='
\echo ''
\echo 'View results:'
\echo '  SELECT * FROM benchmark_summary WHERE test_name LIKE ''%_cascade'';'
\echo ''

-- E-Commerce Cascade Benchmarks
-- Tests cascade scenarios where 1 parent update affects many children
-- Each test approach in separate DO block with automatic rollback

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

-- Approach 1: pg_tviews + jsonb_ivm
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_category_pk INTEGER;
    v_affected_count INTEGER;
    v_new_name TEXT := 'Updated Category ' || (random() * 1000)::INTEGER;
BEGIN
    SELECT c.pk_category, COUNT(*)
    INTO v_category_pk, v_affected_count
    FROM tb_category c
    JOIN tb_product p ON p.fk_category = c.pk_category
    GROUP BY c.pk_category
    ORDER BY COUNT(*) DESC
    LIMIT 1;

    RAISE NOTICE 'Testing cascade: 1 category → % products', v_affected_count;

    v_start := clock_timestamp();

    UPDATE tb_category SET name = v_new_name WHERE pk_category = v_category_pk;

    UPDATE tv_product
    SET data = jsonb_smart_patch_nested(data, jsonb_build_object('name', v_new_name), ARRAY['category']),
        updated_at = now()
    WHERE fk_category = v_category_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark('ecommerce', 'category_cascade', 'small', 'tviews_jsonb_ivm',
                            v_affected_count, 2, v_duration_ms);

    RAISE NOTICE '[1] pg_tviews + jsonb_ivm: %.3f ms (%.3f ms/product)',
        v_duration_ms, v_duration_ms / NULLIF(v_affected_count, 0);

    ROLLBACK;
END $$;

-- Approach 2: Manual + native PG
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_category_pk INTEGER;
    v_affected_count INTEGER;
    v_new_name TEXT := 'Updated Category ' || (random() * 1000)::INTEGER;
BEGIN
    SELECT c.pk_category, COUNT(*)
    INTO v_category_pk, v_affected_count
    FROM tb_category c
    JOIN tb_product p ON p.fk_category = c.pk_category
    GROUP BY c.pk_category
    ORDER BY COUNT(*) DESC
    LIMIT 1;

    v_start := clock_timestamp();

    UPDATE tb_category SET name = v_new_name WHERE pk_category = v_category_pk;

    UPDATE manual_product
    SET data = jsonb_set(data, '{category,name}', to_jsonb(v_new_name)),
        updated_at = now()
    WHERE fk_category = v_category_pk;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark('ecommerce', 'category_cascade', 'small', 'manual_jsonb_set',
                            v_affected_count, 2, v_duration_ms);

    RAISE NOTICE '[2] Manual + jsonb_set: %.3f ms (%.3f ms/product)',
        v_duration_ms, v_duration_ms / NULLIF(v_affected_count, 0);

    ROLLBACK;
END $$;

-- Approach 3: Full refresh
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_category_pk INTEGER;
    v_affected_count INTEGER;
    v_new_name TEXT := 'Updated Category ' || (random() * 1000)::INTEGER;
    v_total_products INTEGER;
BEGIN
    SELECT c.pk_category, COUNT(*)
    INTO v_category_pk, v_affected_count
    FROM tb_category c
    JOIN tb_product p ON p.fk_category = c.pk_category
    GROUP BY c.pk_category
    ORDER BY COUNT(*) DESC
    LIMIT 1;

    SELECT COUNT(*) INTO v_total_products FROM tb_product;

    UPDATE tb_category SET name = v_new_name WHERE pk_category = v_category_pk;

    v_start := clock_timestamp();
    REFRESH MATERIALIZED VIEW CONCURRENTLY mv_product;
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark('ecommerce', 'category_cascade', 'small', 'full_refresh',
                            v_total_products, 2, v_duration_ms);

    RAISE NOTICE '[3] Full refresh: %.3f ms (entire catalog refreshed)',
        v_duration_ms;

    ROLLBACK;
END $$;

\echo ''

-- =============================================================================
-- Test 2: Supplier Info Update (1 → multiple products cascade)
-- =============================================================================

\echo 'Test 2: Supplier info update cascade (1 supplier → multiple products)'
\echo '----------------------------------------------------------------------'

-- Approach 1
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_supplier_pk INTEGER;
    v_affected_count INTEGER;
    v_new_email TEXT := 'updated_' || (random() * 1000)::INTEGER || '@supplier.com';
    v_new_country TEXT := 'Updated Country';
BEGIN
    SELECT s.pk_supplier, COUNT(*)
    INTO v_supplier_pk, v_affected_count
    FROM tb_supplier s
    JOIN tb_product p ON p.fk_supplier = s.pk_supplier
    GROUP BY s.pk_supplier
    ORDER BY COUNT(*) DESC
    LIMIT 1;

    IF v_supplier_pk IS NULL THEN
        RAISE NOTICE 'Skipping supplier cascade - no suppliers with products';
        RETURN;
    END IF;

    RAISE NOTICE 'Testing cascade: 1 supplier → % products', v_affected_count;

    v_start := clock_timestamp();

    UPDATE tb_supplier
    SET contact_email = v_new_email, country = v_new_country
    WHERE pk_supplier = v_supplier_pk;

    UPDATE tv_product tp
    SET data = jsonb_smart_patch_nested(data,
                jsonb_build_object('email', v_new_email, 'country', v_new_country),
                ARRAY['supplier']),
        updated_at = now()
    WHERE pk_product IN (SELECT pk_product FROM tb_product WHERE fk_supplier = v_supplier_pk);

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark('ecommerce', 'supplier_cascade', 'small', 'tviews_jsonb_ivm',
                            v_affected_count, 2, v_duration_ms);

    RAISE NOTICE '[1] pg_tviews + jsonb_ivm: %.3f ms (%.3f ms/product)',
        v_duration_ms, v_duration_ms / NULLIF(v_affected_count, 0);

    ROLLBACK;
END $$;

-- Approach 2
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_supplier_pk INTEGER;
    v_affected_count INTEGER;
    v_new_email TEXT := 'updated_' || (random() * 1000)::INTEGER || '@supplier.com';
    v_new_country TEXT := 'Updated Country';
BEGIN
    SELECT s.pk_supplier, COUNT(*)
    INTO v_supplier_pk, v_affected_count
    FROM tb_supplier s
    JOIN tb_product p ON p.fk_supplier = s.pk_supplier
    GROUP BY s.pk_supplier
    ORDER BY COUNT(*) DESC
    LIMIT 1;

    IF v_supplier_pk IS NULL THEN
        RETURN;
    END IF;

    v_start := clock_timestamp();

    UPDATE tb_supplier
    SET contact_email = v_new_email, country = v_new_country
    WHERE pk_supplier = v_supplier_pk;

    UPDATE manual_product mp
    SET data = jsonb_set(
                jsonb_set(data, '{supplier,email}', to_jsonb(v_new_email)),
                '{supplier,country}', to_jsonb(v_new_country)
            ),
        updated_at = now()
    WHERE pk_product IN (SELECT pk_product FROM tb_product WHERE fk_supplier = v_supplier_pk);

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark('ecommerce', 'supplier_cascade', 'small', 'manual_jsonb_set',
                            v_affected_count, 2, v_duration_ms);

    RAISE NOTICE '[2] Manual + jsonb_set: %.3f ms (%.3f ms/product)',
        v_duration_ms, v_duration_ms / NULLIF(v_affected_count, 0);

    ROLLBACK;
END $$;

-- Approach 3
DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_supplier_pk INTEGER;
    v_affected_count INTEGER;
    v_new_email TEXT := 'updated_' || (random() * 1000)::INTEGER || '@supplier.com';
    v_new_country TEXT := 'Updated Country';
    v_total_products INTEGER;
BEGIN
    SELECT s.pk_supplier, COUNT(*)
    INTO v_supplier_pk, v_affected_count
    FROM tb_supplier s
    JOIN tb_product p ON p.fk_supplier = s.pk_supplier
    GROUP BY s.pk_supplier
    ORDER BY COUNT(*) DESC
    LIMIT 1;

    IF v_supplier_pk IS NULL THEN
        RETURN;
    END IF;

    SELECT COUNT(*) INTO v_total_products FROM tb_product;

    UPDATE tb_supplier
    SET contact_email = v_new_email, country = v_new_country
    WHERE pk_supplier = v_supplier_pk;

    v_start := clock_timestamp();
    REFRESH MATERIALIZED VIEW CONCURRENTLY mv_product;
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    PERFORM record_benchmark('ecommerce', 'supplier_cascade', 'small', 'full_refresh',
                            v_total_products, 2, v_duration_ms);

    RAISE NOTICE '[3] Full refresh: %.3f ms (entire catalog refreshed)',
        v_duration_ms;

    ROLLBACK;
END $$;

\echo ''
\echo '========================================='
\echo 'Cascade benchmarks complete!'
\echo '========================================='
\echo ''
\echo 'View results:'
\echo '  SELECT * FROM benchmark_summary WHERE test_name LIKE ''%_cascade'';'
\echo ''

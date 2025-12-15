-- ========================================
-- 4-WAY PERFORMANCE COMPARISON BENCHMARK
-- ========================================
--
-- Compares 4 different approaches for maintaining denormalized product views:
--
-- 1. pg_tviews + jsonb_ivm   (Transactional views with JSONB IVM)
-- 2. pg_tviews + native      (Transactional views with native PostgreSQL)
-- 3. Manual functions        (Hand-written trigger functions)
-- 4. Full refresh baseline   (Traditional REFRESH MATERIALIZED VIEW)
--
-- This benchmark measures:
-- - Initial data load performance
-- - Incremental UPDATE performance
-- - Incremental INSERT performance
-- - Query performance (reads)
-- - Cascade depth and maintenance overhead
--
-- Usage:
--   psql -d pg_tviews_benchmark -v data_scale="'small'" -f 04_way_comparison.sql
--   psql -d pg_tviews_benchmark -v data_scale="'medium'" -f 04_way_comparison.sql
--   psql -d pg_tviews_benchmark -v data_scale="'large'" -f 04_way_comparison.sql

\timing on
\set QUIET on
\pset format aligned
\pset border 2

-- Determine scale sizes
\if :{?data_scale}
    \set current_scale :data_scale
\else
    \set current_scale 'small'
\endif

\echo ''
\echo '========================================'
\echo '4-WAY BENCHMARK COMPARISON'
\echo '========================================'
\echo ''
\echo 'Data Scale: ' :current_scale
\echo ''

-- Scale configuration
DO $$
DECLARE
    v_scale TEXT := :'current_scale';
    v_num_categories INTEGER;
    v_num_products INTEGER;
    v_num_reviews INTEGER;
BEGIN
    -- Set scale sizes
    CASE v_scale
        WHEN 'small' THEN
            v_num_categories := 20;
            v_num_products := 1000;
            v_num_reviews := 5000;
        WHEN 'medium' THEN
            v_num_categories := 50;
            v_num_products := 10000;
            v_num_reviews := 50000;
        WHEN 'large' THEN
            v_num_categories := 100;
            v_num_products := 100000;
            v_num_reviews := 500000;
        ELSE
            RAISE EXCEPTION 'Unknown scale: %. Use small, medium, or large.', v_scale;
    END CASE;

    -- Store in temp table for use across scenarios
    CREATE TEMP TABLE IF NOT EXISTS benchmark_scale_config (
        scale TEXT PRIMARY KEY,
        num_categories INTEGER,
        num_products INTEGER,
        num_reviews INTEGER
    );

    DELETE FROM benchmark_scale_config WHERE scale = v_scale;

    INSERT INTO benchmark_scale_config VALUES (
        v_scale,
        v_num_categories,
        v_num_products,
        v_num_reviews
    );

    RAISE NOTICE 'Scale %: Categories=%, Products=%, Reviews=%',
        v_scale, v_num_categories, v_num_products, v_num_reviews;
END $$;

-- ========================================
-- SCENARIO 1: pg_tviews + jsonb_ivm
-- ========================================

\echo ''
\echo '----------------------------------------'
\echo 'SCENARIO 1: pg_tviews + jsonb_ivm'
\echo '----------------------------------------'

-- Clean schema
DROP SCHEMA IF EXISTS bench_tviews_jsonb CASCADE;
CREATE SCHEMA bench_tviews_jsonb;
SET search_path TO bench_tviews_jsonb, public;

-- Source tables
\i ../schemas/01_ecommerce_schema.sql

-- Create materialized view using jsonb_ivm
CREATE MATERIALIZED VIEW mv_product_catalog AS
SELECT
    p.pk_product,
    p.fk_category,
    jsonb_build_object(
        'id', p.id,
        'pk', p.pk_product,
        'sku', p.sku,
        'name', p.name,
        'description', p.description,
        'price', jsonb_build_object(
            'base', p.base_price,
            'current', p.current_price,
            'currency', p.currency,
            'discount_pct', ROUND((1 - p.current_price / NULLIF(p.base_price, 0)) * 100, 2)
        ),
        'status', p.status,
        'category', jsonb_build_object(
            'id', c.id,
            'pk', c.pk_category,
            'name', c.name,
            'slug', c.slug
        ),
        'supplier', jsonb_build_object(
            'id', s.id,
            'pk', s.pk_supplier,
            'name', s.name,
            'country', s.country
        ),
        'reviews', jsonb_build_object(
            'count', COALESCE(r.review_count, 0),
            'avg_rating', COALESCE(ROUND(r.avg_rating, 2), 0),
            'verified_count', COALESCE(r.verified_count, 0)
        ),
        'inventory', jsonb_build_object(
            'quantity', COALESCE(i.quantity, 0),
            'reserved', COALESCE(i.reserved, 0),
            'available', COALESCE(i.quantity - i.reserved, 0),
            'in_stock', COALESCE(i.quantity - i.reserved, 0) > 0
        )
    ) AS object_data
FROM tb_product p
INNER JOIN tb_category c ON p.fk_category = c.pk_category
LEFT JOIN tb_supplier s ON p.fk_supplier = s.pk_supplier
LEFT JOIN (
    SELECT
        fk_product,
        COUNT(*) as review_count,
        AVG(rating) as avg_rating,
        COUNT(*) FILTER (WHERE verified_purchase) as verified_count
    FROM tb_review
    GROUP BY fk_product
) r ON p.pk_product = r.fk_product
LEFT JOIN tb_inventory i ON p.pk_product = i.fk_product
WHERE p.status = 'active';

-- Enable pg_tviews with jsonb_ivm
SELECT pg_tviews.enable_tview('mv_product_catalog', jsonb_ivm := true);

-- Benchmark: Initial data load with tviews+jsonb_ivm
\echo '  Loading initial data...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_config RECORD;
BEGIN
    SELECT * INTO v_config FROM benchmark_scale_config WHERE scale = :'current_scale';

    v_start := clock_timestamp();

    -- Generate data inline
    DECLARE
        v_batch_size INTEGER := 1000;
        i INTEGER;
        j INTEGER;
    BEGIN
        -- 1. Generate categories
        INSERT INTO tb_category (name, slug, fk_parent_category)
        SELECT
            'Category ' || i,
            'category-' || i,
            CASE WHEN i > 5 THEN ((i - 1) % 5) + 1 ELSE NULL END
        FROM generate_series(1, v_config.num_categories) AS i;

        -- 2. Generate products
        FOR i IN 1..v_config.num_products BY v_batch_size LOOP
            INSERT INTO tb_product (fk_category, sku, name, description, base_price, current_price, status)
            SELECT
                ((j - 1) % v_config.num_categories) + 1,
                'SKU-' || LPAD(j::TEXT, 10, '0'),
                'Product ' || j,
                'Description for product ' || j || '. ' || repeat('Lorem ipsum. ', 5),
                ROUND((random() * 990 + 10)::NUMERIC, 2),
                ROUND((random() * 990 + 10)::NUMERIC, 2),
                CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
            FROM generate_series(i, LEAST(i + v_batch_size - 1, v_config.num_products)) AS j;
        END LOOP;

        -- 3. Generate inventory
        INSERT INTO tb_inventory (fk_product, quantity, reserved, warehouse_location)
        SELECT
            pk_product,
            (random() * 1000)::INTEGER,
            (random() * 50)::INTEGER,
            'WH-' || (((pk_product - 1) % 10) + 1)
        FROM tb_product;

        -- 4. Generate reviews
        FOR i IN 1..v_config.num_reviews BY v_batch_size LOOP
            INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_count)
            SELECT
                ((j - 1) % v_config.num_products) + 1,
                ((j - 1) % 10000) + 1,
                (random() * 4 + 1)::INTEGER,
                'Review Title ' || j,
                'Review content ' || j || '. ' || repeat('Great product. ', 10),
                random() < 0.7,
                (random() * 100)::INTEGER
            FROM generate_series(i, LEAST(i + v_batch_size - 1, v_config.num_reviews)) AS j;
        END LOOP;
    END;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    -- Record result
    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'initial_load',
        :'current_scale',
        'tviews_jsonb_ivm',
        v_config.num_products,
        v_duration_ms,
        'Full data load with automatic view maintenance'
    );

    RAISE NOTICE '✓ Loaded % products in %.2f ms', v_config.num_products, v_duration_ms;
END $$;

-- Benchmark: Incremental UPDATE (price changes)
\echo '  Testing incremental updates (price changes)...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_rows INTEGER;
BEGIN
    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.9,
        updated_at = now()
    WHERE pk_product % 10 = 0;  -- Update 10% of products

    GET DIAGNOSTICS v_rows = ROW_COUNT;
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'incremental_update',
        :'current_scale',
        'tviews_jsonb_ivm',
        v_rows,
        v_duration_ms,
        '10% price reduction (0.9x multiplier)'
    );

    RAISE NOTICE '✓ Updated % products in %.2f ms (%.3f ms/row)',
        v_rows, v_duration_ms, v_duration_ms / v_rows;
END $$;

-- Benchmark: Incremental INSERT (new products)
\echo '  Testing incremental inserts (new products)...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_rows INTEGER;
BEGIN
    v_start := clock_timestamp();

    INSERT INTO tb_product (fk_category, fk_supplier, sku, name, description, base_price, current_price)
    SELECT
        (SELECT pk_category FROM tb_category ORDER BY random() LIMIT 1),
        (SELECT pk_supplier FROM tb_supplier ORDER BY random() LIMIT 1),
        'NEW-' || generate_series(1, 100),
        'New Product ' || generate_series(1, 100),
        'Newly added product',
        99.99,
        89.99;

    GET DIAGNOSTICS v_rows = ROW_COUNT;
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'incremental_insert',
        :'current_scale',
        'tviews_jsonb_ivm',
        v_rows,
        v_duration_ms,
        'Adding 100 new products'
    );

    RAISE NOTICE '✓ Inserted % products in %.2f ms (%.3f ms/row)',
        v_rows, v_duration_ms, v_duration_ms / v_rows;
END $$;

-- Benchmark: Query performance
\echo '  Testing query performance...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_count INTEGER;
BEGIN
    v_start := clock_timestamp();

    SELECT COUNT(*) INTO v_count
    FROM mv_product_catalog
    WHERE object_data->'price'->>'current' IS NOT NULL;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'query_read',
        :'current_scale',
        'tviews_jsonb_ivm',
        v_count,
        v_duration_ms,
        'SELECT COUNT(*) from materialized view'
    );

    RAISE NOTICE '✓ Queried % rows in %.2f ms', v_count, v_duration_ms;
END $$;

\echo '✓ Scenario 1 complete'

-- ========================================
-- SCENARIO 2: pg_tviews + native PostgreSQL
-- ========================================

\echo ''
\echo '----------------------------------------'
\echo 'SCENARIO 2: pg_tviews + native PostgreSQL'
\echo '----------------------------------------'

-- Clean schema
DROP SCHEMA IF EXISTS bench_tviews_native CASCADE;
CREATE SCHEMA bench_tviews_native;
SET search_path TO bench_tviews_native, public;

-- Source tables (same as scenario 1)
\i ../schemas/01_ecommerce_schema.sql

-- Create materialized view (same definition, different backend)
CREATE MATERIALIZED VIEW mv_product_catalog AS
SELECT
    p.pk_product,
    p.fk_category,
    jsonb_build_object(
        'id', p.id,
        'pk', p.pk_product,
        'sku', p.sku,
        'name', p.name,
        'description', p.description,
        'price', jsonb_build_object(
            'base', p.base_price,
            'current', p.current_price,
            'currency', p.currency,
            'discount_pct', ROUND((1 - p.current_price / NULLIF(p.base_price, 0)) * 100, 2)
        ),
        'status', p.status,
        'category', jsonb_build_object(
            'id', c.id,
            'pk', c.pk_category,
            'name', c.name,
            'slug', c.slug
        ),
        'supplier', jsonb_build_object(
            'id', s.id,
            'pk', s.pk_supplier,
            'name', s.name,
            'country', s.country
        ),
        'reviews', jsonb_build_object(
            'count', COALESCE(r.review_count, 0),
            'avg_rating', COALESCE(ROUND(r.avg_rating, 2), 0),
            'verified_count', COALESCE(r.verified_count, 0)
        ),
        'inventory', jsonb_build_object(
            'quantity', COALESCE(i.quantity, 0),
            'reserved', COALESCE(i.reserved, 0),
            'available', COALESCE(i.quantity - i.reserved, 0),
            'in_stock', COALESCE(i.quantity - i.reserved, 0) > 0
        )
    ) AS object_data
FROM tb_product p
INNER JOIN tb_category c ON p.fk_category = c.pk_category
LEFT JOIN tb_supplier s ON p.fk_supplier = s.pk_supplier
LEFT JOIN (
    SELECT
        fk_product,
        COUNT(*) as review_count,
        AVG(rating) as avg_rating,
        COUNT(*) FILTER (WHERE verified_purchase) as verified_count
    FROM tb_review
    GROUP BY fk_product
) r ON p.pk_product = r.fk_product
LEFT JOIN tb_inventory i ON p.pk_product = i.fk_product
WHERE p.status = 'active';

-- Enable pg_tviews WITHOUT jsonb_ivm
SELECT pg_tviews.enable_tview('mv_product_catalog', jsonb_ivm := false);

-- Benchmark: Initial data load with tviews+native
\echo '  Loading initial data...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_config RECORD;
BEGIN
    SELECT * INTO v_config FROM benchmark_scale_config WHERE scale = :'current_scale';

    v_start := clock_timestamp();

    -- Generate data inline (same data as scenario 1)
    DECLARE
        v_batch_size INTEGER := 1000;
        i INTEGER;
        j INTEGER;
    BEGIN
        -- 1. Generate categories
        INSERT INTO tb_category (name, slug, fk_parent_category)
        SELECT
            'Category ' || i,
            'category-' || i,
            CASE WHEN i > 5 THEN ((i - 1) % 5) + 1 ELSE NULL END
        FROM generate_series(1, v_config.num_categories) AS i;

        -- 2. Generate products
        FOR i IN 1..v_config.num_products BY v_batch_size LOOP
            INSERT INTO tb_product (fk_category, sku, name, description, base_price, current_price, status)
            SELECT
                ((j - 1) % v_config.num_categories) + 1,
                'SKU-' || LPAD(j::TEXT, 10, '0'),
                'Product ' || j,
                'Description for product ' || j || '. ' || repeat('Lorem ipsum. ', 5),
                ROUND((random() * 990 + 10)::NUMERIC, 2),
                ROUND((random() * 990 + 10)::NUMERIC, 2),
                CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
            FROM generate_series(i, LEAST(i + v_batch_size - 1, v_config.num_products)) AS j;
        END LOOP;

        -- 3. Generate inventory
        INSERT INTO tb_inventory (fk_product, quantity, reserved, warehouse_location)
        SELECT
            pk_product,
            (random() * 1000)::INTEGER,
            (random() * 50)::INTEGER,
            'WH-' || (((pk_product - 1) % 10) + 1)
        FROM tb_product;

        -- 4. Generate reviews
        FOR i IN 1..v_config.num_reviews BY v_batch_size LOOP
            INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_count)
            SELECT
                ((j - 1) % v_config.num_products) + 1,
                ((j - 1) % 10000) + 1,
                (random() * 4 + 1)::INTEGER,
                'Review Title ' || j,
                'Review content ' || j || '. ' || repeat('Great product. ', 10),
                random() < 0.7,
                (random() * 100)::INTEGER
            FROM generate_series(i, LEAST(i + v_batch_size - 1, v_config.num_reviews)) AS j;
        END LOOP;
    END;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'initial_load',
        :'current_scale',
        'tviews_native_pg',
        v_config.num_products,
        v_duration_ms,
        'Full data load with native PostgreSQL aggregates'
    );

    RAISE NOTICE '✓ Loaded % products in %.2f ms', v_config.num_products, v_duration_ms;
END $$;

-- Benchmark: Incremental UPDATE
\echo '  Testing incremental updates (price changes)...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_rows INTEGER;
BEGIN
    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.9,
        updated_at = now()
    WHERE pk_product % 10 = 0;

    GET DIAGNOSTICS v_rows = ROW_COUNT;
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'incremental_update',
        :'current_scale',
        'tviews_native_pg',
        v_rows,
        v_duration_ms,
        '10% price reduction (0.9x multiplier)'
    );

    RAISE NOTICE '✓ Updated % products in %.2f ms (%.3f ms/row)',
        v_rows, v_duration_ms, v_duration_ms / v_rows;
END $$;

-- Benchmark: Incremental INSERT
\echo '  Testing incremental inserts (new products)...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_rows INTEGER;
BEGIN
    v_start := clock_timestamp();

    INSERT INTO tb_product (fk_category, fk_supplier, sku, name, description, base_price, current_price)
    SELECT
        (SELECT pk_category FROM tb_category ORDER BY random() LIMIT 1),
        (SELECT pk_supplier FROM tb_supplier ORDER BY random() LIMIT 1),
        'NEW-' || generate_series(1, 100),
        'New Product ' || generate_series(1, 100),
        'Newly added product',
        99.99,
        89.99;

    GET DIAGNOSTICS v_rows = ROW_COUNT;
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'incremental_insert',
        :'current_scale',
        'tviews_native_pg',
        v_rows,
        v_duration_ms,
        'Adding 100 new products'
    );

    RAISE NOTICE '✓ Inserted % products in %.2f ms (%.3f ms/row)',
        v_rows, v_duration_ms, v_duration_ms / v_rows;
END $$;

-- Benchmark: Query performance
\echo '  Testing query performance...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_count INTEGER;
BEGIN
    v_start := clock_timestamp();

    SELECT COUNT(*) INTO v_count
    FROM mv_product_catalog
    WHERE object_data->'price'->>'current' IS NOT NULL;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'query_read',
        :'current_scale',
        'tviews_native_pg',
        v_count,
        v_duration_ms,
        'SELECT COUNT(*) from materialized view'
    );

    RAISE NOTICE '✓ Queried % rows in %.2f ms', v_count, v_duration_ms;
END $$;

\echo '✓ Scenario 2 complete'

-- ========================================
-- SCENARIO 3: Manual Functions (Hand-written triggers)
-- ========================================

\echo ''
\echo '----------------------------------------'
\echo 'SCENARIO 3: Manual Functions'
\echo '----------------------------------------'

-- Clean schema
DROP SCHEMA IF EXISTS bench_manual CASCADE;
CREATE SCHEMA bench_manual;
SET search_path TO bench_manual, public;

-- Source tables
\i ../schemas/01_ecommerce_schema.sql

-- Create denormalized table (manually maintained)
CREATE TABLE mv_product_catalog (
    pk_product INTEGER PRIMARY KEY,
    fk_category INTEGER,
    object_data JSONB NOT NULL
);

CREATE INDEX idx_mv_product_catalog_category ON mv_product_catalog(fk_category);
CREATE INDEX idx_mv_product_catalog_data ON mv_product_catalog USING gin(object_data);

-- Manual refresh function
CREATE OR REPLACE FUNCTION refresh_product_catalog()
RETURNS void AS $$
BEGIN
    -- Truncate and rebuild (full refresh approach for manual)
    TRUNCATE mv_product_catalog;

    INSERT INTO mv_product_catalog (pk_product, fk_category, object_data)
    SELECT
        p.pk_product,
        p.fk_category,
        jsonb_build_object(
            'id', p.id,
            'pk', p.pk_product,
            'sku', p.sku,
            'name', p.name,
            'description', p.description,
            'price', jsonb_build_object(
                'base', p.base_price,
                'current', p.current_price,
                'currency', p.currency,
                'discount_pct', ROUND((1 - p.current_price / NULLIF(p.base_price, 0)) * 100, 2)
            ),
            'status', p.status,
            'category', jsonb_build_object(
                'id', c.id,
                'pk', c.pk_category,
                'name', c.name,
                'slug', c.slug
            ),
            'supplier', jsonb_build_object(
                'id', s.id,
                'pk', s.pk_supplier,
                'name', s.name,
                'country', s.country
            ),
            'reviews', jsonb_build_object(
                'count', COALESCE(r.review_count, 0),
                'avg_rating', COALESCE(ROUND(r.avg_rating, 2), 0),
                'verified_count', COALESCE(r.verified_count, 0)
            ),
            'inventory', jsonb_build_object(
                'quantity', COALESCE(i.quantity, 0),
                'reserved', COALESCE(i.reserved, 0),
                'available', COALESCE(i.quantity - i.reserved, 0),
                'in_stock', COALESCE(i.quantity - i.reserved, 0) > 0
            )
        )
    FROM tb_product p
    INNER JOIN tb_category c ON p.fk_category = c.pk_category
    LEFT JOIN tb_supplier s ON p.fk_supplier = s.pk_supplier
    LEFT JOIN (
        SELECT
            fk_product,
            COUNT(*) as review_count,
            AVG(rating) as avg_rating,
            COUNT(*) FILTER (WHERE verified_purchase) as verified_count
        FROM tb_review
        GROUP BY fk_product
    ) r ON p.pk_product = r.fk_product
    LEFT JOIN tb_inventory i ON p.pk_product = i.fk_product
    WHERE p.status = 'active';
END;
$$ LANGUAGE plpgsql;

-- Benchmark: Initial data load + manual refresh
\echo '  Loading initial data and refreshing...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_config RECORD;
BEGIN
    SELECT * INTO v_config FROM benchmark_scale_config WHERE scale = :'current_scale';

    v_start := clock_timestamp();

    -- Generate data inline
    DECLARE
        v_batch_size INTEGER := 1000;
        i INTEGER;
        j INTEGER;
    BEGIN
        -- 1. Generate categories
        INSERT INTO tb_category (name, slug, fk_parent_category)
        SELECT
            'Category ' || i,
            'category-' || i,
            CASE WHEN i > 5 THEN ((i - 1) % 5) + 1 ELSE NULL END
        FROM generate_series(1, v_config.num_categories) AS i;

        -- 2. Generate products
        FOR i IN 1..v_config.num_products BY v_batch_size LOOP
            INSERT INTO tb_product (fk_category, sku, name, description, base_price, current_price, status)
            SELECT
                ((j - 1) % v_config.num_categories) + 1,
                'SKU-' || LPAD(j::TEXT, 10, '0'),
                'Product ' || j,
                'Description for product ' || j || '. ' || repeat('Lorem ipsum. ', 5),
                ROUND((random() * 990 + 10)::NUMERIC, 2),
                ROUND((random() * 990 + 10)::NUMERIC, 2),
                CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
            FROM generate_series(i, LEAST(i + v_batch_size - 1, v_config.num_products)) AS j;
        END LOOP;

        -- 3. Generate inventory
        INSERT INTO tb_inventory (fk_product, quantity, reserved, warehouse_location)
        SELECT
            pk_product,
            (random() * 1000)::INTEGER,
            (random() * 50)::INTEGER,
            'WH-' || (((pk_product - 1) % 10) + 1)
        FROM tb_product;

        -- 4. Generate reviews
        FOR i IN 1..v_config.num_reviews BY v_batch_size LOOP
            INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_count)
            SELECT
                ((j - 1) % v_config.num_products) + 1,
                ((j - 1) % 10000) + 1,
                (random() * 4 + 1)::INTEGER,
                'Review Title ' || j,
                'Review content ' || j || '. ' || repeat('Great product. ', 10),
                random() < 0.7,
                (random() * 100)::INTEGER
            FROM generate_series(i, LEAST(i + v_batch_size - 1, v_config.num_reviews)) AS j;
        END LOOP;
    END;

    -- Manual refresh required
    PERFORM refresh_product_catalog();

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'initial_load',
        :'current_scale',
        'manual_func',
        v_config.num_products,
        v_duration_ms,
        'Data load + manual full refresh'
    );

    RAISE NOTICE '✓ Loaded % products in %.2f ms', v_config.num_products, v_duration_ms;
END $$;

-- Benchmark: UPDATE + manual refresh
\echo '  Testing updates with manual refresh...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_rows INTEGER;
BEGIN
    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.9,
        updated_at = now()
    WHERE pk_product % 10 = 0;

    GET DIAGNOSTICS v_rows = ROW_COUNT;

    -- Manual refresh required
    PERFORM refresh_product_catalog();

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'incremental_update',
        :'current_scale',
        'manual_func',
        v_rows,
        v_duration_ms,
        '10% price update + full manual refresh'
    );

    RAISE NOTICE '✓ Updated % products in %.2f ms (%.3f ms/row)',
        v_rows, v_duration_ms, v_duration_ms / v_rows;
END $$;

-- Benchmark: INSERT + manual refresh
\echo '  Testing inserts with manual refresh...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_rows INTEGER;
BEGIN
    v_start := clock_timestamp();

    INSERT INTO tb_product (fk_category, fk_supplier, sku, name, description, base_price, current_price)
    SELECT
        (SELECT pk_category FROM tb_category ORDER BY random() LIMIT 1),
        (SELECT pk_supplier FROM tb_supplier ORDER BY random() LIMIT 1),
        'NEW-' || generate_series(1, 100),
        'New Product ' || generate_series(1, 100),
        'Newly added product',
        99.99,
        89.99;

    GET DIAGNOSTICS v_rows = ROW_COUNT;

    -- Manual refresh required
    PERFORM refresh_product_catalog();

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'incremental_insert',
        :'current_scale',
        'manual_func',
        v_rows,
        v_duration_ms,
        '100 inserts + full manual refresh'
    );

    RAISE NOTICE '✓ Inserted % products in %.2f ms (%.3f ms/row)',
        v_rows, v_duration_ms, v_duration_ms / v_rows;
END $$;

-- Benchmark: Query performance
\echo '  Testing query performance...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_count INTEGER;
BEGIN
    v_start := clock_timestamp();

    SELECT COUNT(*) INTO v_count
    FROM mv_product_catalog
    WHERE object_data->'price'->>'current' IS NOT NULL;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'query_read',
        :'current_scale',
        'manual_func',
        v_count,
        v_duration_ms,
        'SELECT COUNT(*) from manually maintained table'
    );

    RAISE NOTICE '✓ Queried % rows in %.2f ms', v_count, v_duration_ms;
END $$;

\echo '✓ Scenario 3 complete'

-- ========================================
-- SCENARIO 4: Full Refresh Baseline
-- ========================================

\echo ''
\echo '----------------------------------------'
\echo 'SCENARIO 4: Full Refresh Baseline'
\echo '----------------------------------------'

-- Clean schema
DROP SCHEMA IF EXISTS bench_full_refresh CASCADE;
CREATE SCHEMA bench_full_refresh;
SET search_path TO bench_full_refresh, public;

-- Source tables
\i ../schemas/01_ecommerce_schema.sql

-- Traditional materialized view (no automatic refresh)
CREATE MATERIALIZED VIEW mv_product_catalog AS
SELECT
    p.pk_product,
    p.fk_category,
    jsonb_build_object(
        'id', p.id,
        'pk', p.pk_product,
        'sku', p.sku,
        'name', p.name,
        'description', p.description,
        'price', jsonb_build_object(
            'base', p.base_price,
            'current', p.current_price,
            'currency', p.currency,
            'discount_pct', ROUND((1 - p.current_price / NULLIF(p.base_price, 0)) * 100, 2)
        ),
        'status', p.status,
        'category', jsonb_build_object(
            'id', c.id,
            'pk', c.pk_category,
            'name', c.name,
            'slug', c.slug
        ),
        'supplier', jsonb_build_object(
            'id', s.id,
            'pk', s.pk_supplier,
            'name', s.name,
            'country', s.country
        ),
        'reviews', jsonb_build_object(
            'count', COALESCE(r.review_count, 0),
            'avg_rating', COALESCE(ROUND(r.avg_rating, 2), 0),
            'verified_count', COALESCE(r.verified_count, 0)
        ),
        'inventory', jsonb_build_object(
            'quantity', COALESCE(i.quantity, 0),
            'reserved', COALESCE(i.reserved, 0),
            'available', COALESCE(i.quantity - i.reserved, 0),
            'in_stock', COALESCE(i.quantity - i.reserved, 0) > 0
        )
    ) AS object_data
FROM tb_product p
INNER JOIN tb_category c ON p.fk_category = c.pk_category
LEFT JOIN tb_supplier s ON p.fk_supplier = s.pk_supplier
LEFT JOIN (
    SELECT
        fk_product,
        COUNT(*) as review_count,
        AVG(rating) as avg_rating,
        COUNT(*) FILTER (WHERE verified_purchase) as verified_count
    FROM tb_review
    GROUP BY fk_product
) r ON p.pk_product = r.fk_product
LEFT JOIN tb_inventory i ON p.pk_product = i.fk_product
WHERE p.status = 'active';

-- Benchmark: Initial data load + full refresh
\echo '  Loading initial data and refreshing...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_config RECORD;
BEGIN
    SELECT * INTO v_config FROM benchmark_scale_config WHERE scale = :'current_scale';

    v_start := clock_timestamp();

    -- Generate data inline
    DECLARE
        v_batch_size INTEGER := 1000;
        i INTEGER;
        j INTEGER;
    BEGIN
        -- 1. Generate categories
        INSERT INTO tb_category (name, slug, fk_parent_category)
        SELECT
            'Category ' || i,
            'category-' || i,
            CASE WHEN i > 5 THEN ((i - 1) % 5) + 1 ELSE NULL END
        FROM generate_series(1, v_config.num_categories) AS i;

        -- 2. Generate products
        FOR i IN 1..v_config.num_products BY v_batch_size LOOP
            INSERT INTO tb_product (fk_category, sku, name, description, base_price, current_price, status)
            SELECT
                ((j - 1) % v_config.num_categories) + 1,
                'SKU-' || LPAD(j::TEXT, 10, '0'),
                'Product ' || j,
                'Description for product ' || j || '. ' || repeat('Lorem ipsum. ', 5),
                ROUND((random() * 990 + 10)::NUMERIC, 2),
                ROUND((random() * 990 + 10)::NUMERIC, 2),
                CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
            FROM generate_series(i, LEAST(i + v_batch_size - 1, v_config.num_products)) AS j;
        END LOOP;

        -- 3. Generate inventory
        INSERT INTO tb_inventory (fk_product, quantity, reserved, warehouse_location)
        SELECT
            pk_product,
            (random() * 1000)::INTEGER,
            (random() * 50)::INTEGER,
            'WH-' || (((pk_product - 1) % 10) + 1)
        FROM tb_product;

        -- 4. Generate reviews
        FOR i IN 1..v_config.num_reviews BY v_batch_size LOOP
            INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_count)
            SELECT
                ((j - 1) % v_config.num_products) + 1,
                ((j - 1) % 10000) + 1,
                (random() * 4 + 1)::INTEGER,
                'Review Title ' || j,
                'Review content ' || j || '. ' || repeat('Great product. ', 10),
                random() < 0.7,
                (random() * 100)::INTEGER
            FROM generate_series(i, LEAST(i + v_batch_size - 1, v_config.num_reviews)) AS j;
        END LOOP;
    END;

    -- Full refresh required
    REFRESH MATERIALIZED VIEW mv_product_catalog;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'initial_load',
        :'current_scale',
        'full_refresh',
        v_config.num_products,
        v_duration_ms,
        'Data load + REFRESH MATERIALIZED VIEW'
    );

    RAISE NOTICE '✓ Loaded % products in %.2f ms', v_config.num_products, v_duration_ms;
END $$;

-- Benchmark: UPDATE + full refresh
\echo '  Testing updates with full refresh...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_rows INTEGER;
BEGIN
    v_start := clock_timestamp();

    UPDATE tb_product
    SET current_price = current_price * 0.9,
        updated_at = now()
    WHERE pk_product % 10 = 0;

    GET DIAGNOSTICS v_rows = ROW_COUNT;

    -- Full refresh required
    REFRESH MATERIALIZED VIEW mv_product_catalog;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'incremental_update',
        :'current_scale',
        'full_refresh',
        v_rows,
        v_duration_ms,
        '10% price update + REFRESH MATERIALIZED VIEW'
    );

    RAISE NOTICE '✓ Updated % products in %.2f ms (%.3f ms/row)',
        v_rows, v_duration_ms, v_duration_ms / v_rows;
END $$;

-- Benchmark: INSERT + full refresh
\echo '  Testing inserts with full refresh...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_rows INTEGER;
BEGIN
    v_start := clock_timestamp();

    INSERT INTO tb_product (fk_category, fk_supplier, sku, name, description, base_price, current_price)
    SELECT
        (SELECT pk_category FROM tb_category ORDER BY random() LIMIT 1),
        (SELECT pk_supplier FROM tb_supplier ORDER BY random() LIMIT 1),
        'NEW-' || generate_series(1, 100),
        'New Product ' || generate_series(1, 100),
        'Newly added product',
        99.99,
        89.99;

    GET DIAGNOSTICS v_rows = ROW_COUNT;

    -- Full refresh required
    REFRESH MATERIALIZED VIEW mv_product_catalog;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'incremental_insert',
        :'current_scale',
        'full_refresh',
        v_rows,
        v_duration_ms,
        '100 inserts + REFRESH MATERIALIZED VIEW'
    );

    RAISE NOTICE '✓ Inserted % products in %.2f ms (%.3f ms/row)',
        v_rows, v_duration_ms, v_duration_ms / v_rows;
END $$;

-- Benchmark: Query performance
\echo '  Testing query performance...'

DO $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_count INTEGER;
BEGIN
    v_start := clock_timestamp();

    SELECT COUNT(*) INTO v_count
    FROM mv_product_catalog
    WHERE object_data->'price'->>'current' IS NOT NULL;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, execution_time_ms, notes
    ) VALUES (
        'ecommerce',
        'query_read',
        :'current_scale',
        'full_refresh',
        v_count,
        v_duration_ms,
        'SELECT COUNT(*) from materialized view'
    );

    RAISE NOTICE '✓ Queried % rows in %.2f ms', v_count, v_duration_ms;
END $$;

\echo '✓ Scenario 4 complete'

-- ========================================
-- COMPARISON REPORT
-- ========================================

\echo ''
\echo '========================================'
\echo 'COMPARISON REPORT'
\echo '========================================'
\echo ''

-- Reset search path for querying results
RESET search_path;

-- Summary by operation type
\echo '--- Performance by Operation Type ---'
\echo ''

SELECT
    test_name AS operation,
    operation_type AS approach,
    data_scale AS scale,
    ROUND(execution_time_ms, 2) AS time_ms,
    rows_affected,
    ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) AS ms_per_row,
    notes
FROM benchmark_results
WHERE scenario = 'ecommerce'
    AND data_scale = :'current_scale'
ORDER BY test_name, execution_time_ms;

\echo ''
\echo '--- Performance Improvements vs Baseline ---'
\echo ''

WITH baseline AS (
    SELECT
        test_name,
        data_scale,
        execution_time_ms AS baseline_ms
    FROM benchmark_results
    WHERE operation_type = 'full_refresh'
        AND scenario = 'ecommerce'
        AND data_scale = :'current_scale'
)
SELECT
    r.test_name AS operation,
    r.operation_type AS approach,
    ROUND(r.execution_time_ms, 2) AS time_ms,
    ROUND(b.baseline_ms, 2) AS baseline_ms,
    ROUND(b.baseline_ms / NULLIF(r.execution_time_ms, 0), 2) || 'x' AS speedup,
    ROUND(b.baseline_ms - r.execution_time_ms, 2) AS time_saved_ms,
    ROUND((b.baseline_ms - r.execution_time_ms) / NULLIF(b.baseline_ms, 0) * 100, 1) || '%' AS improvement_pct
FROM benchmark_results r
JOIN baseline b USING (test_name, data_scale)
WHERE r.operation_type != 'full_refresh'
    AND r.scenario = 'ecommerce'
    AND r.data_scale = :'current_scale'
ORDER BY r.test_name, r.execution_time_ms;

\echo ''
\echo '--- Head-to-Head: pg_tviews+jsonb_ivm vs pg_tviews+native ---'
\echo ''

WITH jsonb AS (
    SELECT test_name, execution_time_ms AS jsonb_ms
    FROM benchmark_results
    WHERE operation_type = 'tviews_jsonb_ivm'
        AND scenario = 'ecommerce'
        AND data_scale = :'current_scale'
),
native AS (
    SELECT test_name, execution_time_ms AS native_ms
    FROM benchmark_results
    WHERE operation_type = 'tviews_native_pg'
        AND scenario = 'ecommerce'
        AND data_scale = :'current_scale'
)
SELECT
    j.test_name AS operation,
    ROUND(j.jsonb_ms, 2) AS jsonb_delta_ms,
    ROUND(n.native_ms, 2) AS native_pg_ms,
    CASE
        WHEN j.jsonb_ms < n.native_ms THEN 'jsonb_delta ' || ROUND(n.native_ms / j.jsonb_ms, 2) || 'x faster'
        WHEN n.native_ms < j.jsonb_ms THEN 'native_pg ' || ROUND(j.jsonb_ms / n.native_ms, 2) || 'x faster'
        ELSE 'tie'
    END AS winner,
    ROUND(ABS(j.jsonb_ms - n.native_ms), 2) AS diff_ms
FROM jsonb j
JOIN native n USING (test_name)
ORDER BY j.test_name;

\echo ''
\echo '========================================'
\echo 'BENCHMARK COMPLETE'
\echo '========================================'
\echo ''
\echo 'Scale: ' :current_scale
\echo 'Results saved to: benchmark_results table'
\echo ''

\timing off
\set QUIET off

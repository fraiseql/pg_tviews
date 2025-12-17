-- E-Commerce Test Data Generation
-- Supports multiple scales: small, medium, large
-- Uses trinity pattern: id (UUID), pk_{entity} (INTEGER), fk_{entity} (INTEGER)

-- Ensure we're inserting into the benchmark schema
SET search_path TO benchmark, public;

-- Configuration via psql variables (set before running)
-- \set data_scale 'small'  -- options: small, medium, large

-- Data scale is set via sed substitution from run_benchmarks.sh

-- Scale definitions:
-- small:  10 categories, 1K products, 5K reviews
-- medium: 100 categories, 100K products, 500K reviews
-- large:  500 categories, 1M products, 5M reviews

DO $$
DECLARE
    v_scale TEXT := __DATA_SCALE__;
    v_num_categories INTEGER;
    v_num_products INTEGER;
    v_num_reviews INTEGER;
    v_batch_size INTEGER := 1000;
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
BEGIN
    -- Ensure we're in the right schema
    SET LOCAL search_path TO benchmark, public;

    -- Clear existing data for clean benchmark runs
    TRUNCATE TABLE tb_review CASCADE;
    TRUNCATE TABLE tb_inventory CASCADE;
    TRUNCATE TABLE tb_product CASCADE;
    TRUNCATE TABLE tb_category CASCADE;
    TRUNCATE TABLE tv_product CASCADE;

    -- Reset sequences for consistent IDs
    ALTER SEQUENCE tb_category_pk_category_seq RESTART WITH 1;
    ALTER SEQUENCE tb_product_pk_product_seq RESTART WITH 1;
    ALTER SEQUENCE tb_review_pk_review_seq RESTART WITH 1;
    ALTER SEQUENCE tb_inventory_pk_inventory_seq RESTART WITH 1;

    v_start := clock_timestamp();

    -- Set scale parameters
    CASE v_scale
        WHEN 'small' THEN
            v_num_categories := 10;
            v_num_products := 1000;
            v_num_reviews := 5000;
        WHEN 'medium' THEN
            v_num_categories := 100;
            v_num_products := 100000;
            v_num_reviews := 500000;
        WHEN 'large' THEN
            v_num_categories := 500;
            v_num_products := 1000000;
            v_num_reviews := 5000000;
        ELSE
            RAISE EXCEPTION 'Invalid data_scale: %. Use small, medium, or large.', v_scale;
    END CASE;

    RAISE NOTICE 'Generating % scale data: % categories, % products, % reviews',
        v_scale, v_num_categories, v_num_products, v_num_reviews;

    -- 1. Generate categories
    RAISE NOTICE 'Generating categories...';
    -- Insert all categories without parent references first
    INSERT INTO tb_category (name, slug)
    SELECT
        'Category ' || i,
        'category-' || i
    FROM generate_series(1, v_num_categories) AS i;

    -- Then update child categories to have parents
    -- Temporarily disable constraint checking for this update
    SET LOCAL session_replication_role = 'replica';
    UPDATE tb_category
    SET fk_parent_category = ((pk_category - 1) % 5) + 1
    WHERE pk_category > 5;
    SET LOCAL session_replication_role = 'origin';

    -- 2. Generate products in batches
    RAISE NOTICE 'Generating products...';
    -- Disable constraints for bulk insert
    SET LOCAL session_replication_role = 'replica';
    FOR i IN 1..v_num_products BY v_batch_size LOOP
        INSERT INTO tb_product (fk_category, sku, name, description, base_price, current_price, status)
        SELECT
            ((j - 1) % v_num_categories) + 1,  -- Distribute across categories
            'SKU-' || LPAD(j::TEXT, 10, '0'),
            'Product ' || j,
            'Description for product ' || j || '. ' || repeat('Lorem ipsum dolor sit amet. ', 10),
            ROUND((random() * 990 + 10)::NUMERIC, 2),  -- Base price: $10-$1000
            ROUND((random() * 990 + 10)::NUMERIC, 2),  -- Current price
            CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
        FROM generate_series(i, LEAST(i + v_batch_size - 1, v_num_products)) AS j;

        IF i % 10000 = 1 THEN
            RAISE NOTICE '  Products: % / %', LEAST(i + v_batch_size - 1, v_num_products), v_num_products;
        END IF;
    END LOOP;
    -- Re-enable constraints
    SET LOCAL session_replication_role = 'origin';

    -- 3. Generate inventory
    RAISE NOTICE 'Generating inventory...';
    INSERT INTO tb_inventory (fk_product, quantity, reserved, warehouse_location)
    SELECT
        pk_product,
        (random() * 1000)::INTEGER,  -- 0-1000 units
        (random() * 50)::INTEGER,    -- 0-50 reserved
        'WH-' || (((pk_product - 1) % 10) + 1)  -- 10 warehouses
    FROM tb_product;

    -- 4. Generate reviews in batches
    RAISE NOTICE 'Generating reviews...';
    FOR i IN 1..v_num_reviews BY v_batch_size LOOP
        INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_count)
        SELECT
            ((j - 1) % v_num_products) + 1,  -- Distribute across products
            ((j - 1) % 10000) + 1,  -- 10K unique users
            (random() * 4 + 1)::INTEGER,  -- Rating 1-5
            'Review Title ' || j,
            'Review content ' || j || '. ' || repeat('This product is great. ', 15),
            random() < 0.7,  -- 70% verified purchases
            (random() * 100)::INTEGER  -- 0-100 helpful votes
        FROM generate_series(i, LEAST(i + v_batch_size - 1, v_num_reviews)) AS j;

        IF i % 50000 = 1 THEN
            RAISE NOTICE '  Reviews: % / %', LEAST(i + v_batch_size - 1, v_num_reviews), v_num_reviews;
        END IF;
    END LOOP;

    v_end := clock_timestamp();

    -- 5. TVIEW table needs to be populated (Approach 1: pg_tviews)
    -- Note: CREATE TABLE ... AS SELECT creates the table with initial data,
    -- but we still need to convert it to a TVIEW for automatic incremental refresh
    RAISE NOTICE 'Populating TVIEW (pg_tviews)...';
    -- The tv_product table was already created with data by the schema
    -- But we need to verify it has rows (it should from CREATE TABLE AS SELECT)
     IF (SELECT COUNT(*) FROM tv_product) = 0 THEN
         INSERT INTO tv_product SELECT id, pk_product, fk_category, data FROM v_product;
     END IF;

    -- 6. Populate manual table (Approach 2: manual updates)
    RAISE NOTICE 'Populating manual table...';
    PERFORM refresh_manual_product();

    -- 7. Populate manual function table (Approach 3: generic refresh)
    RAISE NOTICE 'Populating manual function table...';
    PERFORM refresh_manual_func_product();

    -- 8. Populate materialized view (Approach 4: full refresh)
    RAISE NOTICE 'Populating materialized view...';
    REFRESH MATERIALIZED VIEW mv_product;

    RAISE NOTICE 'Data generation complete in %.2f seconds',
        EXTRACT(EPOCH FROM (v_end - v_start));

    -- Verify counts
    RAISE NOTICE '';
    RAISE NOTICE 'Data verification:';
    RAISE NOTICE '  Categories: %', (SELECT COUNT(*) FROM tb_category);
    RAISE NOTICE '  Products: %', (SELECT COUNT(*) FROM tb_product);
    RAISE NOTICE '  Reviews: %', (SELECT COUNT(*) FROM tb_review);
    RAISE NOTICE '  Inventory records: %', (SELECT COUNT(*) FROM tb_inventory);
    RAISE NOTICE '  TVIEW rows (pg_tviews): %', (SELECT COUNT(*) FROM tv_product);
    RAISE NOTICE '  Manual table rows: %', (SELECT COUNT(*) FROM manual_product);
    RAISE NOTICE '  Manual function table rows: %', (SELECT COUNT(*) FROM manual_func_product);
    RAISE NOTICE '  Materialized view rows: %', (SELECT COUNT(*) FROM mv_product);

    RAISE NOTICE '';
    RAISE NOTICE 'Ready to run benchmarks!';
END $$;

-- Analyze tables for query planning
ANALYZE tb_category;
ANALYZE tb_product;
ANALYZE tb_review;
ANALYZE tb_inventory;
ANALYZE tv_product;
ANALYZE manual_product;
ANALYZE manual_func_product;
ANALYZE mv_product;

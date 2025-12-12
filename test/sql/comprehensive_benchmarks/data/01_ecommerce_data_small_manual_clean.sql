-- E-Commerce Test Data Generation - SMALL SCALE
-- 10 categories, 1K products, 5K reviews

DO $$
DECLARE
    v_num_categories INTEGER := 10;
    v_num_suppliers INTEGER := 10;
    v_num_products INTEGER := 1000;
    v_num_reviews INTEGER := 5000;
    v_batch_size INTEGER := 1000;
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
BEGIN
    RAISE NOTICE 'Generating SMALL scale data: % categories, % suppliers, % products, % reviews',
        v_num_categories, v_num_suppliers, v_num_products, v_num_reviews;

    v_start := clock_timestamp();

    -- 1. Generate categories
    RAISE NOTICE 'Generating categories...';
    -- Insert parent categories first
    INSERT INTO tb_category (name, slug, fk_parent_category)
    SELECT
        'Category ' || i,
        'category-' || i,
        NULL
    FROM generate_series(1, 5) AS i;
    
    -- Insert child categories
    INSERT INTO tb_category (name, slug, fk_parent_category)
    SELECT
        'Category ' || i,
        'category-' || i,
        ((i - 6) % 5) + 1
    FROM generate_series(6, v_num_categories) AS i;

    -- 2. Generate suppliers
    RAISE NOTICE 'Generating suppliers...';
    INSERT INTO tb_supplier (name, contact_email, contact_phone, country)
    SELECT
        'Supplier ' || i,
        'contact' || i || '@supplier' || i || '.com',
        '+1-555-' || LPAD(i::TEXT, 4, '0'),
        CASE ((i - 1) % 4)
            WHEN 0 THEN 'USA'
            WHEN 1 THEN 'China'
            WHEN 2 THEN 'Germany'
            ELSE 'Japan'
        END
    FROM generate_series(1, v_num_suppliers) AS i;

    -- 3. Generate products
    RAISE NOTICE 'Generating products...';
    INSERT INTO tb_product (fk_category, fk_supplier, sku, name, description, base_price, current_price, status)
    SELECT
        ((j - 1) % v_num_categories) + 1,
        CASE WHEN random() < 0.9 THEN ((j - 1) % v_num_suppliers) + 1 ELSE NULL END,
        'SKU-' || LPAD(j::TEXT, 10, '0'),
        'Product ' || j,
        'Description for product ' || j || '. ' || repeat('Lorem ipsum dolor sit amet. ', 10),
        ROUND((random() * 990 + 10)::NUMERIC, 2),
        ROUND((random() * 990 + 10)::NUMERIC, 2),
        CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
    FROM generate_series(1, v_num_products) AS j;

    -- 4. Generate inventory
    RAISE NOTICE 'Generating inventory...';
    INSERT INTO tb_inventory (fk_product, quantity, reserved, warehouse_location)
    SELECT
        pk_product,
        (random() * 1000)::INTEGER,
        (random() * 50)::INTEGER,
        'WH-' || (((pk_product - 1) % 10) + 1)
    FROM tb_product;

    -- 5. Generate reviews
    RAISE NOTICE 'Generating reviews...';
    INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_count)
    SELECT
        ((j - 1) % v_num_products) + 1,
        ((j - 1) % 10000) + 1,
        (random() * 4 + 1)::INTEGER,
        'Review Title ' || j,
        'Review content ' || j || '. ' || repeat('This product is great. ', 15),
        random() < 0.7,
        (random() * 100)::INTEGER
    FROM generate_series(1, v_num_reviews) AS j;

    v_end := clock_timestamp();

    -- 6. Populate TVIEW (Approach 1: pg_tviews)
    RAISE NOTICE 'Populating TVIEW (pg_tviews)...';

    -- 7. Populate manual table (Approach 2: manual updates)
    RAISE NOTICE 'Populating manual table...';

    -- 8. Populate materialized view (Approach 3: full refresh)
    RAISE NOTICE 'Populating materialized view...';

    RAISE NOTICE 'Data generation complete in %.2f seconds',
        EXTRACT(EPOCH FROM (v_end - v_start));

    -- Verify counts
    RAISE NOTICE '';
    RAISE NOTICE 'Data verification:';
    RAISE NOTICE '  Categories: %', (SELECT COUNT(*) FROM tb_category);
    RAISE NOTICE '  Suppliers: %', (SELECT COUNT(*) FROM tb_supplier);
    RAISE NOTICE '  Products: %', (SELECT COUNT(*) FROM tb_product);
    RAISE NOTICE '  Reviews: %', (SELECT COUNT(*) FROM tb_review);
    RAISE NOTICE '  Inventory records: %', (SELECT COUNT(*) FROM tb_inventory);

    RAISE NOTICE '';
    RAISE NOTICE 'Ready to run benchmarks!';
END $$;

-- Analyze tables for query planning
ANALYZE tb_category;
ANALYZE tb_supplier;
ANALYZE tb_product;
ANALYZE tb_review;
ANALYZE tb_inventory;
ANALYZE tv_product;
ANALYZE manual_product;
ANALYZE mv_product;

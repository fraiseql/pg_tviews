-- E-Commerce Data Generation (Manual Setup Version)
-- Modified to work without pg_tviews extension
-- Generates data for approaches 3 & 4 only

-- Set scale parameters
\set data_scale 'small'

DO $$
DECLARE
    v_num_categories INTEGER := 10;
    v_num_suppliers INTEGER := 10;
    v_num_products INTEGER := 1000;
    v_num_reviews INTEGER := 5000;
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
BEGIN
    RAISE NOTICE 'Generating % scale data: % categories, % suppliers, % products, % reviews',
        :'data_scale', v_num_categories, v_num_suppliers, v_num_products, v_num_reviews;

    v_start := clock_timestamp();

    -- 1. Generate categories (parents first, then children)
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
        ((i - 6) % 5) + (SELECT MIN(pk_category) FROM tb_category WHERE fk_parent_category IS NULL)
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
        (SELECT pk_category FROM tb_category ORDER BY random() LIMIT 1),
        CASE WHEN random() < 0.8 THEN (SELECT pk_supplier FROM tb_supplier ORDER BY random() LIMIT 1) ELSE NULL END,
        'SKU-' || LPAD(i::TEXT, 6, '0'),
        'Product ' || i,
        'Description for product ' || i || '. This is a high-quality product with excellent features.',
        (random() * 500 + 10)::NUMERIC(10, 2),
        (random() * 500 + 10)::NUMERIC(10, 2),
        CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
    FROM generate_series(1, v_num_products) AS i;

    -- 4. Generate inventory
    RAISE NOTICE 'Generating inventory...';
    INSERT INTO tb_inventory (fk_product, quantity_available, reorder_point, warehouse_location)
    SELECT
        pk_product,
        (random() * 1000)::INTEGER,
        (random() * 100 + 10)::INTEGER,
        'WH-' || chr(65 + (random() * 5)::INTEGER)  -- Random warehouse A-E
    FROM tb_product;

    -- 5. Generate reviews
    RAISE NOTICE 'Generating reviews...';
    INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_votes)
    SELECT
        (SELECT pk_product FROM tb_product ORDER BY random() LIMIT 1),
        (random() * 10000 + 1)::INTEGER,
        (random() * 4 + 1)::INTEGER,
        'Review Title ' || j,
        'Review content ' || j || '. ' || repeat('This product is great. ', 15),
        random() < 0.7,
        (random() * 100)::INTEGER
    FROM generate_series(1, v_num_reviews) AS j;

    v_end := clock_timestamp();

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
    RAISE NOTICE 'Ready to run manual benchmarks!';
END $$;

-- Analyze tables for query planning
ANALYZE tb_category;
ANALYZE tb_supplier;
ANALYZE tb_product;
ANALYZE tb_review;
ANALYZE tb_inventory;
-- E-Commerce Data Generation (Manual Setup Version - Large Scale)
-- Modified to work without pg_tviews extension
-- Generates data for approaches 3 & 4 only: 500 categories, 1M products, 5M reviews
-- WARNING: This will take 10-30 minutes and require ~8-16GB RAM

DO $$
DECLARE
    v_num_categories INTEGER := 500;
    v_num_suppliers INTEGER := 200;
    v_num_products INTEGER := 1000000;
    v_num_reviews INTEGER := 5000000;
    v_batch_size INTEGER := 10000;
    v_progress NUMERIC;
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_current_count INTEGER;
BEGIN
    RAISE NOTICE 'Generating LARGE scale data: % categories, % suppliers, % products, % reviews',
        v_num_categories, v_num_suppliers, v_num_products, v_num_reviews;
    RAISE NOTICE 'WARNING: This may take 10-30 minutes and requires ~8-16GB RAM';
    RAISE NOTICE '         Ensure sufficient disk space (20GB+) and patience!';

    v_start := clock_timestamp();

    -- 1. Generate categories (parents first, then children)
    RAISE NOTICE 'Generating categories...';
    -- Insert parent categories first (top level)
    INSERT INTO tb_category (name, slug, fk_parent_category)
    SELECT
        'Category ' || i,
        'category-' || i,
        NULL
    FROM generate_series(1, 100) AS i;

    -- Insert child categories (with parents)
    INSERT INTO tb_category (name, slug, fk_parent_category)
    SELECT
        'Category ' || i,
        'category-' || i,
        ((i - 101) % 100) + (SELECT MIN(pk_category) FROM tb_category WHERE fk_parent_category IS NULL)
    FROM generate_series(101, v_num_categories) AS i;

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

    -- 3. Generate products (in batches for progress reporting)
    RAISE NOTICE 'Generating products (this will take several minutes)...';
    FOR i IN 0..(v_num_products / v_batch_size - 1) LOOP
        INSERT INTO tb_product (fk_category, fk_supplier, sku, name, description, base_price, current_price, status)
        SELECT
            (SELECT pk_category FROM tb_category ORDER BY random() LIMIT 1),
            CASE WHEN random() < 0.8 THEN (SELECT pk_supplier FROM tb_supplier ORDER BY random() LIMIT 1) ELSE NULL END,
            'SKU-' || LPAD((i * v_batch_size + j)::TEXT, 10, '0'),
            'Product ' || (i * v_batch_size + j),
            'Description for product ' || (i * v_batch_size + j) || '. This is a high-quality product with excellent features and specifications.',
            (random() * 500 + 10)::NUMERIC(10, 2),
            (random() * 500 + 10)::NUMERIC(10, 2),
            CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
        FROM generate_series(1, v_batch_size) AS j;

        v_progress := ((i + 1) * v_batch_size)::NUMERIC / v_num_products * 100;
        RAISE NOTICE 'Products: %.1f%% complete (%/% batches)', v_progress, i + 1, v_num_products / v_batch_size;
    END LOOP;

    -- Handle remaining products
    v_current_count := (SELECT COUNT(*) FROM tb_product);
    IF v_current_count < v_num_products THEN
        INSERT INTO tb_product (fk_category, fk_supplier, sku, name, description, base_price, current_price, status)
        SELECT
            (SELECT pk_category FROM tb_category ORDER BY random() LIMIT 1),
            CASE WHEN random() < 0.8 THEN (SELECT pk_supplier FROM tb_supplier ORDER BY random() LIMIT 1) ELSE NULL END,
            'SKU-' || LPAD((v_current_count + j)::TEXT, 10, '0'),
            'Product ' || (v_current_count + j),
            'Description for product ' || (v_current_count + j) || '. This is a high-quality product with excellent features and specifications.',
            (random() * 500 + 10)::NUMERIC(10, 2),
            (random() * 500 + 10)::NUMERIC(10, 2),
            CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
        FROM generate_series(1, v_num_products - v_current_count) AS j;
    END IF;

    -- 4. Generate inventory
    RAISE NOTICE 'Generating inventory...';
    INSERT INTO tb_inventory (fk_product, quantity_available, reorder_point, warehouse_location)
    SELECT
        pk_product,
        (random() * 1000)::INTEGER,
        (random() * 100 + 10)::INTEGER,
        'WH-' || chr(65 + (random() * 5)::INTEGER)  -- Random warehouse A-E
    FROM tb_product;

    -- 5. Generate reviews (in batches for progress reporting)
    RAISE NOTICE 'Generating reviews (this will take several minutes)...';
    FOR i IN 0..(v_num_reviews / v_batch_size - 1) LOOP
        INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_votes)
        SELECT
            (SELECT pk_product FROM tb_product ORDER BY random() LIMIT 1),
            (random() * 1000000 + 1)::INTEGER,
            (random() * 4 + 1)::INTEGER,
            'Review Title ' || (i * v_batch_size + j),
            'Review content ' || (i * v_batch_size + j) || '. ' || repeat('This product is excellent and highly recommended. ', 20),
            random() < 0.7,
            (random() * 100)::INTEGER
        FROM generate_series(1, v_batch_size) AS j;

        v_progress := ((i + 1) * v_batch_size)::NUMERIC / v_num_reviews * 100;
        RAISE NOTICE 'Reviews: %.1f%% complete (%/% batches)', v_progress, i + 1, v_num_reviews / v_batch_size;
    END LOOP;

    -- Handle remaining reviews
    v_current_count := (SELECT COUNT(*) FROM tb_review);
    IF v_current_count < v_num_reviews THEN
        INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_votes)
        SELECT
            (SELECT pk_product FROM tb_product ORDER BY random() LIMIT 1),
            (random() * 1000000 + 1)::INTEGER,
            (random() * 4 + 1)::INTEGER,
            'Review Title ' || (v_current_count + j),
            'Review content ' || (v_current_count + j) || '. ' || repeat('This product is excellent and highly recommended. ', 20),
            random() < 0.7,
            (random() * 100)::INTEGER
        FROM generate_series(1, v_num_reviews - v_current_count) AS j;
    END IF;

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    RAISE NOTICE 'Data generation complete in %.2f seconds (%.1f minutes)',
        v_duration_ms / 1000, v_duration_ms / 60000;

    -- Verify counts
    RAISE NOTICE '';
    RAISE NOTICE 'Data verification:';
    RAISE NOTICE '  Categories: %', (SELECT COUNT(*) FROM tb_category);
    RAISE NOTICE '  Suppliers: %', (SELECT COUNT(*) FROM tb_supplier);
    RAISE NOTICE '  Products: %', (SELECT COUNT(*) FROM tb_product);
    RAISE NOTICE '  Reviews: %', (SELECT COUNT(*) FROM tb_review);
    RAISE NOTICE '  Inventory records: %', (SELECT COUNT(*) FROM tb_inventory);

    RAISE NOTICE '';
    RAISE NOTICE 'Ready to run large scale manual benchmarks!';
    RAISE NOTICE 'Note: Performance will be slower due to data volume.';
END $$;

-- Analyze tables for query planning (this may take time on large datasets)
ANALYZE tb_category;
ANALYZE tb_supplier;
ANALYZE tb_product;
ANALYZE tb_review;
ANALYZE tb_inventory;
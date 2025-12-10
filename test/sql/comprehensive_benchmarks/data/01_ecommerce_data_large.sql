-- E-Commerce Large Scale Data (1M products)
-- Distribution: 500 categories, 200 suppliers, 1M products, 5M reviews
-- WARNING: This will take several minutes and require ~2-4GB RAM

\echo ''
\echo '========================================='
\echo 'LARGE SCALE DATA GENERATION (1M)'
\echo '========================================='
\echo ''
\echo 'WARNING: This may take 5-10 minutes'
\echo '         Requires ~2-4GB available RAM'
\echo ''

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
    RAISE NOTICE 'Target dataset:';
    RAISE NOTICE '  Categories: %', v_num_categories;
    RAISE NOTICE '  Suppliers: %', v_num_suppliers;
    RAISE NOTICE '  Products: %', v_num_products;
    RAISE NOTICE '  Reviews: %', v_num_reviews;
    RAISE NOTICE '  Batch size: %', v_batch_size;
    RAISE NOTICE '';

    v_start := clock_timestamp();

    -- =========================================================================
    -- Generate Categories
    -- =========================================================================

    RAISE NOTICE '1. Generating categories...';
    INSERT INTO tb_category (name, slug, fk_parent_category)
    SELECT
        'Category ' || i,
        'category-' || i,
        CASE
            WHEN i > 100 THEN ((i - 1) % 100) + 1  -- 80% have parent category
            ELSE NULL
        END
    FROM generate_series(1, v_num_categories) AS i;

    RAISE NOTICE '   ✓ Created % categories', v_num_categories;
    RAISE NOTICE '';

    -- =========================================================================
    -- Generate Suppliers
    -- =========================================================================

    RAISE NOTICE '2. Generating suppliers...';
    INSERT INTO tb_supplier (name, contact_email, contact_phone, country)
    SELECT
        'Supplier ' || i,
        'contact' || i || '@supplier' || ((i - 1) % 10 + 1) || '.com',
        '+1-555-' || LPAD(i::TEXT, 4, '0'),
        CASE ((i - 1) % 10)
            WHEN 0 THEN 'USA'
            WHEN 1 THEN 'China'
            WHEN 2 THEN 'Germany'
            WHEN 3 THEN 'Japan'
            WHEN 4 THEN 'South Korea'
            WHEN 5 THEN 'Taiwan'
            WHEN 6 THEN 'Vietnam'
            WHEN 7 THEN 'Mexico'
            WHEN 8 THEN 'India'
            ELSE 'Brazil'
        END
    FROM generate_series(1, v_num_suppliers) AS i;

    RAISE NOTICE '   ✓ Created % suppliers', v_num_suppliers;
    RAISE NOTICE '';

    -- =========================================================================
    -- Generate Products (in batches with progress)
    -- =========================================================================

    RAISE NOTICE '3. Generating products (% batches of %)...',
        CEIL(v_num_products::NUMERIC / v_batch_size), v_batch_size;

    FOR i IN 1..v_num_products BY v_batch_size LOOP
        INSERT INTO tb_product (
            fk_category,
            fk_supplier,
            sku,
            name,
            description,
            base_price,
            current_price,
            status
        )
        SELECT
            ((j - 1) % v_num_categories) + 1,  -- Distribute across categories
            CASE
                WHEN random() < 0.9 THEN ((j - 1) % v_num_suppliers) + 1
                ELSE NULL  -- 10% products have no supplier
            END,
            'SKU-' || LPAD(j::TEXT, 10, '0'),
            'Product ' || j,
            'Description for product ' || j || '. ' ||
                CASE ((j - 1) % 4)
                    WHEN 0 THEN 'High quality craftsmanship. Long lasting durability.'
                    WHEN 1 THEN 'Best value for money. Customer satisfaction guaranteed.'
                    WHEN 2 THEN 'Premium materials used. Exceptional performance.'
                    ELSE 'Customer favorite. Top rated product.'
                END,
            ROUND((random() * 990 + 10)::NUMERIC, 2),
            ROUND((random() * 990 + 10)::NUMERIC, 2),
            CASE
                WHEN random() < 0.9 THEN 'active'
                WHEN random() < 0.95 THEN 'inactive'
                ELSE 'discontinued'
            END
        FROM generate_series(
            i,
            LEAST(i + v_batch_size - 1, v_num_products)
        ) AS j;

        -- Progress indicator every 50k rows
        IF i % 50000 = 1 OR i + v_batch_size > v_num_products THEN
            v_progress := (LEAST(i + v_batch_size - 1, v_num_products)::NUMERIC / v_num_products) * 100;
            v_end := clock_timestamp();
            v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;
            RAISE NOTICE '   Progress: %.1f%% (% / %) - %.1f sec elapsed',
                v_progress,
                LEAST(i + v_batch_size - 1, v_num_products),
                v_num_products,
                v_duration_ms / 1000;
        END IF;
    END LOOP;

    RAISE NOTICE '   ✓ Created % products', v_num_products;
    RAISE NOTICE '';

    -- =========================================================================
    -- Generate Inventory (1:1 with products)
    -- =========================================================================

    RAISE NOTICE '4. Generating inventory records...';

    FOR i IN 1..v_num_products BY v_batch_size LOOP
        INSERT INTO tb_inventory (fk_product, quantity, reserved, warehouse_location)
        SELECT
            pk_product,
            FLOOR(random() * 1000)::INTEGER,
            FLOOR(random() * 50)::INTEGER,
            'WH-' || LPAD((((pk_product - 1) % 20) + 1)::TEXT, 2, '0')
        FROM tb_product
        WHERE pk_product >= i AND pk_product < i + v_batch_size;

        IF i % 100000 = 1 OR i + v_batch_size > v_num_products THEN
            v_progress := (LEAST(i + v_batch_size - 1, v_num_products)::NUMERIC / v_num_products) * 100;
            RAISE NOTICE '   Progress: %.1f%%', v_progress;
        END IF;
    END LOOP;

    RAISE NOTICE '   ✓ Created % inventory records', v_num_products;
    RAISE NOTICE '';

    -- =========================================================================
    -- Generate Reviews (avg 5 per product)
    -- =========================================================================

    RAISE NOTICE '5. Generating reviews (% batches of %)...',
        CEIL(v_num_reviews::NUMERIC / v_batch_size), v_batch_size;

    FOR i IN 1..v_num_reviews BY v_batch_size LOOP
        INSERT INTO tb_review (
            fk_product,
            fk_user,
            rating,
            title,
            content,
            verified_purchase,
            helpful_count
        )
        SELECT
            ((j - 1) % v_num_products) + 1,  -- Distribute reviews across products
            ((j - 1) % 50000) + 1,  -- 50K unique users
            FLOOR(random() * 5 + 1)::INTEGER,  -- 1-5 stars
            CASE FLOOR(random() * 5)
                WHEN 0 THEN 'Excellent product!'
                WHEN 1 THEN 'Good value'
                WHEN 2 THEN 'Average quality'
                WHEN 3 THEN 'Not satisfied'
                ELSE 'Amazing!'
            END,
            'Review content for product. ' ||
                CASE FLOOR(random() * 3)
                    WHEN 0 THEN 'Highly recommend to others.'
                    WHEN 1 THEN 'Could be better for the price.'
                    ELSE 'Perfect for my specific needs.'
                END,
            random() < 0.7,  -- 70% verified purchases
            FLOOR(random() * 100)::INTEGER
        FROM generate_series(
            i,
            LEAST(i + v_batch_size - 1, v_num_reviews)
        ) AS j;

        IF i % 500000 = 1 OR i + v_batch_size > v_num_reviews THEN
            v_progress := (LEAST(i + v_batch_size - 1, v_num_reviews)::NUMERIC / v_num_reviews) * 100;
            v_end := clock_timestamp();
            v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;
            RAISE NOTICE '   Progress: %.1f%% (% / %) - %.1f sec elapsed',
                v_progress,
                LEAST(i + v_batch_size - 1, v_num_reviews),
                v_num_reviews,
                v_duration_ms / 1000;
        END IF;
    END LOOP;

    RAISE NOTICE '   ✓ Created % reviews', v_num_reviews;
    RAISE NOTICE '';

    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    -- =========================================================================
    -- Run ANALYZE for query optimization
    -- =========================================================================

    RAISE NOTICE '6. Running ANALYZE on tables...';
    ANALYZE tb_category;
    ANALYZE tb_supplier;
    ANALYZE tb_product;
    ANALYZE tb_review;
    ANALYZE tb_inventory;
    RAISE NOTICE '   ✓ Statistics updated';
    RAISE NOTICE '';

    -- =========================================================================
    -- Populate materialized tables
    -- =========================================================================

    RAISE NOTICE '7. Populating materialized tables (this may take several minutes)...';

    RAISE NOTICE '   Populating tv_product...';
    v_start := clock_timestamp();
    PERFORM refresh_tv_product();
    v_end := clock_timestamp();
    RAISE NOTICE '   ✓ tv_product complete (%.1f sec)',
        EXTRACT(EPOCH FROM (v_end - v_start));

    RAISE NOTICE '   Populating manual_product...';
    v_start := clock_timestamp();
    PERFORM refresh_manual_product();
    v_end := clock_timestamp();
    RAISE NOTICE '   ✓ manual_product complete (%.1f sec)',
        EXTRACT(EPOCH FROM (v_end - v_start));

    RAISE NOTICE '   Populating mv_product...';
    v_start := clock_timestamp();
    REFRESH MATERIALIZED VIEW mv_product;
    v_end := clock_timestamp();
    RAISE NOTICE '   ✓ mv_product complete (%.1f sec)',
        EXTRACT(EPOCH FROM (v_end - v_start));

    RAISE NOTICE '';

    -- =========================================================================
    -- Summary
    -- =========================================================================

    RAISE NOTICE '========================================';
    RAISE NOTICE 'Data generation complete!';
    RAISE NOTICE '========================================';
    RAISE NOTICE 'Total time: %.2f seconds (%.2f minutes)',
        v_duration_ms / 1000, v_duration_ms / 60000;
    RAISE NOTICE '';
    RAISE NOTICE 'Dataset statistics:';
    RAISE NOTICE '  Categories: %', (SELECT COUNT(*) FROM tb_category);
    RAISE NOTICE '  Suppliers: %', (SELECT COUNT(*) FROM tb_supplier);
    RAISE NOTICE '  Products: %', (SELECT COUNT(*) FROM tb_product);
    RAISE NOTICE '  Reviews: %', (SELECT COUNT(*) FROM tb_review);
    RAISE NOTICE '  Inventory: %', (SELECT COUNT(*) FROM tb_inventory);
    RAISE NOTICE '  tv_product rows: %', (SELECT COUNT(*) FROM tv_product);
    RAISE NOTICE '  manual_product rows: %', (SELECT COUNT(*) FROM manual_product);
    RAISE NOTICE '  mv_product rows: %', (SELECT COUNT(*) FROM mv_product);
    RAISE NOTICE '';

    -- Distribution check
    RAISE NOTICE 'Distribution check:';
    RAISE NOTICE '  Avg products/category: %.1f',
        (SELECT AVG(cnt) FROM (
            SELECT COUNT(*) as cnt FROM tb_product GROUP BY fk_category
        ) x);
    RAISE NOTICE '  Avg products/supplier: %.1f',
        (SELECT AVG(cnt) FROM (
            SELECT COUNT(*) as cnt FROM tb_product WHERE fk_supplier IS NOT NULL GROUP BY fk_supplier
        ) x);
    RAISE NOTICE '  Avg reviews/product: %.1f',
        (SELECT AVG(cnt) FROM (
            SELECT COUNT(*) as cnt FROM tb_review GROUP BY fk_product
        ) x);
    RAISE NOTICE '  Category with most products: % products',
        (SELECT COUNT(*) as cnt FROM tb_product GROUP BY fk_category ORDER BY cnt DESC LIMIT 1);
    RAISE NOTICE '';

END $$;

\echo 'Ready for large scale benchmarks!'
\echo 'Run: \i scenarios/01_ecommerce_benchmarks_large.sql'
\echo ''

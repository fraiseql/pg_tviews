-- Inline E-Commerce Data Generation Template
-- This provides reusable inline SQL for generating test data
-- To use: Copy this DO block and adjust schema names as needed

-- Parameters: v_num_categories, v_num_products, v_num_reviews
-- Schema prefix: adjust table names as needed (e.g., bench_tviews_jsonb.tb_category)

DO $$
DECLARE
    v_num_categories INTEGER := 20;   -- Set from config
    v_num_products INTEGER := 1000;   -- Set from config
    v_num_reviews INTEGER := 5000;    -- Set from config
    v_batch_size INTEGER := 1000;
BEGIN
    -- 1. Generate categories
    INSERT INTO tb_category (name, slug, fk_parent_category)
    SELECT
        'Category ' || i,
        'category-' || i,
        CASE WHEN i > 5 THEN ((i - 1) % 5) + 1 ELSE NULL END
    FROM generate_series(1, v_num_categories) AS i;

    -- 2. Generate products
    FOR i IN 1..v_num_products BY v_batch_size LOOP
        INSERT INTO tb_product (fk_category, sku, name, description, base_price, current_price, status)
        SELECT
            ((j - 1) % v_num_categories) + 1,
            'SKU-' || LPAD(j::TEXT, 10, '0'),
            'Product ' || j,
            'Description for product ' || j || '. ' || repeat('Lorem ipsum dolor sit amet. ', 5),
            ROUND((random() * 990 + 10)::NUMERIC, 2),
            ROUND((random() * 990 + 10)::NUMERIC, 2),
            CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
        FROM generate_series(i, LEAST(i + v_batch_size - 1, v_num_products)) AS j;
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
    FOR i IN 1..v_num_reviews BY v_batch_size LOOP
        INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_count)
        SELECT
            ((j - 1) % v_num_products) + 1,
            ((j - 1) % 10000) + 1,
            (random() * 4 + 1)::INTEGER,
            'Review Title ' || j,
            'Review content ' || j || '. ' || repeat('This product is great. ', 10),
            random() < 0.7,
            (random() * 100)::INTEGER
        FROM generate_series(i, LEAST(i + v_batch_size - 1, v_num_reviews)) AS j;
    END LOOP;
END $$;

-- Real benchmark — data generator.
--
-- Parameterised via psql -v:
--   n_categories, n_suppliers, n_products, n_reviews
-- All FKs stay in range by construction (modulo the parent count).

INSERT INTO tb_category (name, slug)
SELECT 'Category ' || g, 'category-' || g
FROM generate_series(1, :n_categories) g;

INSERT INTO tb_supplier (name, country)
SELECT 'Supplier ' || g, (ARRAY['US', 'DE', 'FR', 'CN', 'JP'])[1 + (g % 5)]
FROM generate_series(1, :n_suppliers) g;

INSERT INTO tb_product (fk_category, fk_supplier, sku, name, description, base_price, current_price, status)
SELECT
    1 + (g % :n_categories),
    1 + (g % :n_suppliers),
    'SKU-' || lpad(g::text, 10, '0'),
    'Product ' || g,
    'Description for product ' || g || '. ' || repeat('Lorem ipsum. ', 5),
    round((random() * 990 + 10)::numeric, 2),
    round((random() * 990 + 10)::numeric, 2),
    CASE WHEN random() < 0.9 THEN 'active' ELSE 'inactive' END
FROM generate_series(1, :n_products) g;

INSERT INTO tb_inventory (fk_product, quantity, reserved, warehouse_location)
SELECT pk_product, (random() * 1000)::int, (random() * 50)::int, 'WH-' || (1 + (pk_product % 10))
FROM tb_product;

INSERT INTO tb_review (fk_product, fk_user, rating, title, content, verified_purchase, helpful_count)
SELECT
    1 + (g % :n_products),
    1 + (g % 10000),
    1 + (random() * 4)::int,
    'Review Title ' || g,
    'Review content ' || g || '. ' || repeat('Great product. ', 10),
    random() < 0.7,
    (random() * 100)::int
FROM generate_series(1, :n_reviews) g;

ANALYZE;

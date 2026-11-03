-- With the patch path off, every write recomputes the TVIEW rows it reaches from
-- the backing view: the path a patch falls back to. This file drives that path
-- through the shapes a fallback must handle.
--
-- 1. An order document that nests its customer (LEFT JOIN), and aggregates its
--    items, each nesting its product: writes to each of the four base tables,
--    an item DELETE and an order moved to another customer keep it fresh.
-- 2. A TVIEW created by entity name over a table off the naming convention
--    (pk_test AS pk_fallback): INSERT, UPDATE, DELETE.
-- 3. The CREATE TABLE tv_<entity> AS form, with an aggregated child table.
-- DROP TABLE of each TVIEW unregisters it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_recompute_cascades.sql
-- expect-output: recompute_cascades: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

SET pg_tviews.direct_patch_enabled = off;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'recompute cascades FAIL: %', what; END IF; END $$;

-- 1. Orders, their customer and their items' products.
CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY, id UUID DEFAULT gen_random_uuid(),
    name TEXT, email TEXT, profile JSONB DEFAULT '{}'::jsonb);
CREATE TABLE tb_product (
    pk_product BIGSERIAL PRIMARY KEY, id UUID DEFAULT gen_random_uuid(),
    name TEXT, price NUMERIC(10,2), category TEXT);
CREATE TABLE tb_order (
    pk_order BIGSERIAL PRIMARY KEY, id UUID DEFAULT gen_random_uuid(),
    fk_user BIGINT REFERENCES tb_user(pk_user), status TEXT, created_at TIMESTAMPTZ DEFAULT now());
CREATE TABLE tb_order_item (
    pk_order_item BIGSERIAL PRIMARY KEY, id UUID DEFAULT gen_random_uuid(),
    fk_order BIGINT REFERENCES tb_order(pk_order), fk_product BIGINT REFERENCES tb_product(pk_product),
    quantity INT, price_at_order NUMERIC(10,2));

CREATE TABLE tv_order AS
SELECT o.pk_order, o.id, o.fk_user,
       jsonb_build_object(
           'id', o.id, 'status', o.status, 'created_at', o.created_at,
           'customer', jsonb_build_object('id', u.id, 'name', u.name, 'email', u.email, 'profile', u.profile),
           'items', COALESCE(jsonb_agg(jsonb_build_object(
                         'id', oi.id, 'quantity', oi.quantity, 'price', oi.price_at_order,
                         'product', jsonb_build_object('id', p.id, 'name', p.name, 'category', p.category))
                     ORDER BY oi.pk_order_item) FILTER (WHERE oi.pk_order_item IS NOT NULL), '[]'::jsonb)
       ) AS data
FROM tb_order o
LEFT JOIN tb_user u ON u.pk_user = o.fk_user
LEFT JOIN tb_order_item oi ON oi.fk_order = o.pk_order
LEFT JOIN tb_product p ON p.pk_product = oi.fk_product
GROUP BY o.pk_order, o.id, o.status, o.created_at, u.id, u.name, u.email, u.profile;

SELECT must(EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'order'),
            'CREATE TABLE tv_order AS did not register the TVIEW');
SELECT assert_fresh('tv_order', 'pk_order', 'CREATE TABLE tv_order AS');

INSERT INTO tb_user (name, email, profile) VALUES
    ('Alice', 'alice@example.com', '{"theme": "light"}'), ('Bob', 'bob@example.com', '{"theme": "dark"}');
SELECT assert_fresh('tv_order', 'pk_order', 'INSERT INTO tb_user');
INSERT INTO tb_product (name, price, category) VALUES
    ('Widget A', 10.00, 'widgets'), ('Widget B', 20.00, 'widgets'), ('Gadget C', 30.00, 'gadgets');
SELECT assert_fresh('tv_order', 'pk_order', 'INSERT INTO tb_product');
INSERT INTO tb_order (fk_user, status) VALUES (1, 'pending');
SELECT assert_fresh('tv_order', 'pk_order', 'INSERT INTO tb_order');
INSERT INTO tb_order_item (fk_order, fk_product, quantity, price_at_order) VALUES (1, 1, 2, 10.00), (1, 2, 1, 20.00);
SELECT assert_fresh('tv_order', 'pk_order', 'INSERT INTO tb_order_item');
SELECT must((SELECT jsonb_path_query_array(data, '$.items[*].product.name') FROM tv_order WHERE pk_order = 1)
            = '["Widget A", "Widget B"]' AND
            (SELECT data->'customer'->>'name' FROM tv_order WHERE pk_order = 1) = 'Alice',
            'tv_order 1 is not Alice''s order of Widget A and B');
SELECT must((SELECT data->>'id' FROM tv_order WHERE pk_order = 1) = (SELECT id::text FROM tb_order WHERE pk_order = 1),
            'the order id is not in its document');

UPDATE tb_user SET name = 'Alice Smith' WHERE pk_user = 1;
SELECT assert_fresh('tv_order', 'pk_order', 'UPDATE tb_user');
SELECT must((SELECT data->'customer'->>'name' FROM tv_order WHERE pk_order = 1) = 'Alice Smith',
            'customer not refreshed by UPDATE tb_user');

UPDATE tb_product SET category = 'gizmos' WHERE pk_product = 1;
SELECT assert_fresh('tv_order', 'pk_order', 'UPDATE tb_product');
SELECT must((SELECT data->'items'->0->'product'->>'category' FROM tv_order WHERE pk_order = 1) = 'gizmos',
            'item product not refreshed by UPDATE tb_product');

INSERT INTO tb_order (fk_user, status) VALUES (2, 'pending');
INSERT INTO tb_order_item (fk_order, fk_product, quantity, price_at_order) VALUES (2, 3, 4, 30.00);
SELECT assert_fresh('tv_order', 'pk_order', 'a second order with one item');

DELETE FROM tb_order_item WHERE pk_order_item = 1;
SELECT assert_fresh('tv_order', 'pk_order', 'DELETE FROM tb_order_item');
SELECT must((SELECT jsonb_path_query_array(data, '$.items[*].product.name') FROM tv_order WHERE pk_order = 1)
            = '["Widget B"]', 'deleted item still in tv_order 1');

UPDATE tb_order SET fk_user = 2, status = 'shipped' WHERE pk_order = 1;
SELECT assert_fresh('tv_order', 'pk_order', 'UPDATE tb_order');
SELECT must((SELECT count(*) FROM tv_order) = 2
            AND (SELECT data->'customer'->>'name' FROM tv_order WHERE pk_order = 1) = 'Bob'
            AND (SELECT data->>'status' FROM tv_order WHERE pk_order = 1) = 'shipped',
            'tv_order 1 not moved to Bob and shipped');

DROP TABLE tv_order;
SELECT must(NOT EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'order'),
            'order still registered after DROP TABLE tv_order');

-- 2. Created by entity name, over a table off the naming convention.
CREATE TABLE test_fallback (pk_test BIGINT PRIMARY KEY, data JSONB);
INSERT INTO test_fallback VALUES (1, '{"id": "test_123", "items": []}');
SELECT pg_tviews_create('fallback', $$ SELECT pk_test AS pk_fallback, data FROM test_fallback $$);
SELECT assert_fresh('tv_fallback', 'pk_fallback', 'pg_tviews_create');
UPDATE test_fallback SET data = jsonb_set(data, '{items}', data->'items' || '[{"id": "item_1"}]') WHERE pk_test = 1;
SELECT assert_fresh('tv_fallback', 'pk_fallback', 'UPDATE test_fallback');
INSERT INTO test_fallback VALUES (2, '{"id": "test_456", "items": []}');
SELECT assert_fresh('tv_fallback', 'pk_fallback', 'INSERT INTO test_fallback');
DELETE FROM test_fallback WHERE pk_test = 2;
SELECT assert_fresh('tv_fallback', 'pk_fallback', 'DELETE FROM test_fallback');
SELECT must((SELECT data->'items'->0->>'id' FROM tv_fallback WHERE pk_fallback = 1) = 'item_1'
            AND (SELECT count(*) FROM tv_fallback) = 1, 'tv_fallback not refreshed');
SELECT pg_tviews_drop('fallback');

-- 3. CREATE TABLE tv_<entity> AS, with an aggregated child table.
CREATE TABLE tb_author (pk_author BIGINT PRIMARY KEY, id UUID NOT NULL DEFAULT gen_random_uuid(), name TEXT);
CREATE TABLE tb_book (pk_book BIGINT PRIMARY KEY, id UUID NOT NULL DEFAULT gen_random_uuid(),
                      fk_author BIGINT REFERENCES tb_author(pk_author), title TEXT);
INSERT INTO tb_author (pk_author, name) VALUES (1, 'Ada');
INSERT INTO tb_book (pk_book, fk_author, title) VALUES (1, 1, 'Notes');
CREATE TABLE tv_author AS
SELECT a.pk_author, a.id,
       jsonb_build_object('name', a.name,
                          'books', COALESCE(jsonb_agg(b.title ORDER BY b.pk_book)
                                            FILTER (WHERE b.pk_book IS NOT NULL), '[]'::jsonb)) AS data
FROM tb_author a LEFT JOIN tb_book b ON b.fk_author = a.pk_author
GROUP BY a.pk_author, a.id, a.name;
SELECT assert_fresh('tv_author', 'pk_author', 'CREATE TABLE tv_author AS');
INSERT INTO tb_book (pk_book, fk_author, title) VALUES (2, 1, 'Sketch');
SELECT assert_fresh('tv_author', 'pk_author', 'INSERT INTO tb_book');
UPDATE tb_author SET name = 'Ada L.' WHERE pk_author = 1;
SELECT assert_fresh('tv_author', 'pk_author', 'UPDATE tb_author');
SELECT must((SELECT data FROM tv_author WHERE pk_author = 1) = '{"name": "Ada L.", "books": ["Notes", "Sketch"]}',
            'tv_author not refreshed by writes to tb_author and tb_book');
DROP TABLE tv_author;
SELECT must(NOT EXISTS (SELECT 1 FROM pg_tview_meta), 'a TVIEW is still registered after the drops');

\echo 'recompute_cascades: PASS'

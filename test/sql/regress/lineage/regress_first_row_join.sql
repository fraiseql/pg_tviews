-- Regression test for issue #194: a table joined to a non-key column of a
-- first-row subquery (ROW_NUMBER … rn = 1, or DISTINCT ON) is mapped in two hops:
-- the written row → the subquery's rows carrying it → their partition / DISTINCT ON
-- key → the TVIEW key. The subquery's own table keeps mapping through its key: a
-- write that changes which row is first changes the partition it is in. A table
-- linked to the key only through a non-key column of its own first-row level stays
-- untraceable.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_first_row_join.sql
--
-- expect-output: issue #194 first row join: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#194 FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'created';
EXCEPTION WHEN OTHERS THEN RETURN SQLERRM; END $$;
CREATE FUNCTION kinds(e text) RETURNS text LANGUAGE sql AS $$
    SELECT cascade_kinds::text || ' uncascaded=' || uncascaded_tables::text
    FROM tviews.registry WHERE entity = e $$;

SET pg_tviews.uncascaded_policy = 'error';

CREATE TABLE tb_product (pk_product bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_customer (pk_customer bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, fk_customer bigint REFERENCES tb_customer,
                       fk_product bigint, placed_on date, deleted_at timestamptz);
INSERT INTO tb_product VALUES (1, default, 'p1'), (2, default, 'p2'), (3, default, 'p3');
INSERT INTO tb_customer VALUES (1, default, 'c1'), (2, default, 'c2'), (3, default, 'c3');
INSERT INTO tb_order VALUES (10, 1, 1, '2026-01-01', NULL), (11, 1, 2, '2026-02-01', NULL),
                            (20, 2, 2, '2026-01-05', NULL), (30, 3, 3, '2026-03-01', NULL);

-- The issue's view: the product of each customer's first live order.
CREATE VIEW v_first_product_rn AS
SELECT c.pk_customer, c.id, c.name, p.name AS first_product
FROM tb_customer c
LEFT JOIN (SELECT o.fk_customer, o.fk_product,
                  ROW_NUMBER() OVER (PARTITION BY o.fk_customer ORDER BY o.placed_on, o.pk_order) AS rn
           FROM tb_order o WHERE o.deleted_at IS NULL) f ON f.fk_customer = c.pk_customer AND f.rn = 1
LEFT JOIN tb_product p ON p.pk_product = f.fk_product;
-- The same with DISTINCT ON.
CREATE VIEW v_first_product_do AS
SELECT c.pk_customer, c.id, c.name, p.name AS first_product
FROM tb_customer c
LEFT JOIN (SELECT DISTINCT ON (o.fk_customer) o.fk_customer, o.fk_product
           FROM tb_order o WHERE o.deleted_at IS NULL
           ORDER BY o.fk_customer, o.placed_on, o.pk_order) f ON f.fk_customer = c.pk_customer
LEFT JOIN tb_product p ON p.pk_product = f.fk_product;
-- The first-row level inside a view, read by another view.
CREATE VIEW v_first_order AS
SELECT DISTINCT ON (o.fk_customer) o.fk_customer, o.fk_product
FROM tb_order o WHERE o.deleted_at IS NULL ORDER BY o.fk_customer, o.placed_on, o.pk_order;
CREATE VIEW v_first_product_nested AS
SELECT c.pk_customer, c.id, c.name, p.name AS first_product
FROM tb_customer c
LEFT JOIN v_first_order f ON f.fk_customer = c.pk_customer
LEFT JOIN tb_product p ON p.pk_product = f.fk_product;

CREATE FUNCTION exercise(tag text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_product SET name = 'P1' WHERE pk_product = 1;              -- customer 1's first product
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': a first product renamed');
    UPDATE tb_product SET name = 'P2' WHERE pk_product = 2;              -- customer 2's, and 1's second
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': a product renamed');
    UPDATE tb_order SET placed_on = '2026-03-01' WHERE pk_order = 10;    -- 11 becomes 1's first
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': the first row changed by its date');
    UPDATE tb_order SET fk_product = 3 WHERE pk_order = 11;              -- the first row's product
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': the first row''s product changed');
    UPDATE tb_order SET deleted_at = now() WHERE pk_order = 11;          -- 10 is first again
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': the first row soft-deleted');
    DELETE FROM tb_product WHERE pk_product = 1;
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': a first product deleted');
    INSERT INTO tb_product VALUES (1, default, 'p1');
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': a first product inserted');
    -- Back to the start.
    UPDATE tb_product SET name = 'p' || pk_product;
    UPDATE tb_order SET placed_on = '2026-01-01' WHERE pk_order = 10;
    UPDATE tb_order SET fk_product = 2, deleted_at = NULL WHERE pk_order = 11;
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': reset');
END $$;

SELECT tviews.pg_tviews_create('tv_customer', 'SELECT pk_customer, id, name, first_product FROM ' || v)
FROM unnest(ARRAY['v_first_product_rn']) v;
SELECT must((SELECT cascade_kinds ->> 'tb_product' = 'mapped' AND cascade_kinds ->> 'tb_order' = 'local'
             AND uncascaded_tables = '{}' FROM tviews.registry WHERE entity = 'customer'),
            'ROW_NUMBER: ' || kinds('customer'));
SELECT must((SELECT first_product FROM tv_customer WHERE pk_customer = 1) = 'p1', 'initial rows');
SELECT exercise('ROW_NUMBER');
SELECT tviews.pg_tviews_drop('tv_customer');

SELECT tviews.pg_tviews_create('tv_customer', 'SELECT pk_customer, id, name, first_product FROM v_first_product_do');
SELECT must((SELECT cascade_kinds ->> 'tb_product' = 'mapped' AND uncascaded_tables = '{}'
             FROM tviews.registry WHERE entity = 'customer'), 'DISTINCT ON: ' || kinds('customer'));
SELECT exercise('DISTINCT ON');
SELECT tviews.pg_tviews_drop('tv_customer');

SELECT tviews.pg_tviews_create('tv_customer', 'SELECT pk_customer, id, name, first_product FROM v_first_product_nested');
SELECT must((SELECT cascade_kinds ->> 'tb_product' = 'mapped' AND uncascaded_tables = '{}'
             FROM tviews.registry WHERE entity = 'customer'), 'nested: ' || kinds('customer'));
SELECT exercise('nested');
SELECT tviews.pg_tviews_drop('tv_customer');

-- Control: per product, the customers whose first order is of it. tb_order reaches
-- the product only through a non-key column of its own first-row level: a write
-- that changes which order is first changes another order's product, so mapping
-- the written row's fk_product would miss the product it leaves.
SELECT must(error_of($$SELECT tviews.pg_tviews_create('tv_product', $q$
    SELECT p.pk_product, p.id, p.name,
           (SELECT count(*) FROM v_first_order f WHERE f.fk_product = p.pk_product) AS first_buyers
    FROM tb_product p $q$)$$) LIKE '%public.tb_order%', 'the control was not refused');
SET pg_tviews.uncascaded_policy = 'full_refresh';
SELECT tviews.pg_tviews_create('tv_product', $q$
    SELECT p.pk_product, p.id, p.name,
           (SELECT count(*) FROM v_first_order f WHERE f.fk_product = p.pk_product) AS first_buyers
    FROM tb_product p $q$);
SELECT must((SELECT cascade_kinds ->> 'tb_order' = 'all_keys' FROM tviews.registry WHERE entity = 'product'),
            'control: ' || kinds('product'));
UPDATE tb_order SET placed_on = '2026-03-01' WHERE pk_order = 10;
SELECT assert_fresh('tv_product', 'pk_product', 'control: the first row changed');

\echo issue #194 first row join: PASS

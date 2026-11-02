-- Writes two joins away reach the TVIEW through plain (non-TVIEW) tables.
--
-- tv_order aggregates tb_item rows reached through tb_group (tb_item.fk_group ->
-- tb_group.fk_order -> tb_order). An UPDATE, INSERT or DELETE of an item, a bulk
-- item UPDATE, and an UPDATE of the group in between all refresh the right
-- order; an item with a NULL fk_group refreshes nothing and fails nothing; the
-- other order is left alone.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_multihop_leaf_writes.sql
-- expect-output: multihop_leaf_writes: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'multihop leaf writes FAIL: %', what; END IF; END $$;

CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                       customer text NOT NULL);
CREATE TABLE tb_group (pk_group bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                       fk_order bigint NOT NULL REFERENCES tb_order, label text NOT NULL);
CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                      fk_group bigint REFERENCES tb_group, product text NOT NULL,
                      qty int NOT NULL DEFAULT 1);

INSERT INTO tb_order (pk_order, customer) VALUES (1, 'Alice'), (2, 'Bob');
INSERT INTO tb_group (pk_group, fk_order, label) VALUES (10, 1, 'Group A1'), (20, 1, 'Group A2'), (30, 2, 'Group B1');
INSERT INTO tb_item (pk_item, fk_group, product, qty) VALUES
    (100, 10, 'Widget', 2), (101, 10, 'Gadget', 1), (102, 20, 'Bolt', 5),
    (103, 30, 'Nut', 10), (104, 30, 'Washer', 3);

SELECT pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id,
           jsonb_build_object('customer', o.customer,
               'items', COALESCE(jsonb_agg(jsonb_build_object('product', i.product, 'qty', i.qty, 'group', g.label))
                                 FILTER (WHERE i.pk_item IS NOT NULL), '[]'::jsonb)) AS data
    FROM tb_order o
    LEFT JOIN tb_group g ON g.fk_order = o.pk_order
    LEFT JOIN tb_item i ON i.fk_group = g.pk_group
    GROUP BY o.pk_order, o.id, o.customer $$);
SELECT assert_fresh('tv_order', 'pk_order', 'pg_tviews_create');

SELECT must((SELECT jsonb_array_length(plan->'paths') FROM tviews.pg_tview_meta WHERE entity = 'order') >= 1,
            'no local path stored in the order plan');
SELECT must((SELECT count(*) FROM tv_order) = 2
            AND (SELECT jsonb_array_length(data->'items') FROM tv_order WHERE pk_order = 1) = 3,
            'initial population is not 2 orders with 3 items for Alice');

UPDATE tb_item SET product = 'SuperWidget', qty = 99 WHERE pk_item = 100;
SELECT assert_fresh('tv_order', 'pk_order', 'UPDATE of a leaf item');
SELECT must((SELECT data->'items' @> '[{"product": "SuperWidget", "qty": 99}]' FROM tv_order WHERE pk_order = 1),
            'leaf UPDATE did not reach order 1');

INSERT INTO tb_item (pk_item, fk_group, product, qty) VALUES (105, 20, 'Spring', 7);
SELECT assert_fresh('tv_order', 'pk_order', 'INSERT of a leaf item');
SELECT must((SELECT jsonb_array_length(data->'items') FROM tv_order WHERE pk_order = 1) = 4,
            'leaf INSERT did not reach order 1');

DELETE FROM tb_item WHERE pk_item = 105;
SELECT assert_fresh('tv_order', 'pk_order', 'DELETE of a leaf item');
SELECT must((SELECT jsonb_array_length(data->'items') FROM tv_order WHERE pk_order = 1) = 3,
            'leaf DELETE did not reach order 1');

UPDATE tb_group SET label = 'Updated Group A1' WHERE pk_group = 10;
SELECT assert_fresh('tv_order', 'pk_order', 'UPDATE of an intermediate group');
SELECT must((SELECT data->'items' @> '[{"group": "Updated Group A1"}]' FROM tv_order WHERE pk_order = 1),
            'intermediate UPDATE did not reach order 1');

UPDATE tb_item SET qty = qty + 100 WHERE fk_group = 30;
SELECT assert_fresh('tv_order', 'pk_order', 'bulk UPDATE of leaf items');
SELECT must((SELECT sum((e->>'qty')::int) FROM tv_order, jsonb_array_elements(data->'items') e
             WHERE pk_order = 2) = 213,
            'bulk UPDATE: order 2 total qty is not 213');

-- An item with no group maps to no order: nothing refreshed, nothing fails.
INSERT INTO tb_item (pk_item, fk_group, product, qty) VALUES (200, NULL, 'Orphan', 1);
SELECT assert_fresh('tv_order', 'pk_order', 'INSERT of an item with a NULL fk_group');
DELETE FROM tb_item WHERE pk_item = 200;
SELECT assert_fresh('tv_order', 'pk_order', 'DELETE of an item with a NULL fk_group');

SELECT must((SELECT data->>'customer' FROM tv_order WHERE pk_order = 2) = 'Bob',
            'order 2 customer changed by writes to order 1');

SELECT pg_tviews_drop('tv_order');
DROP EXTENSION pg_tviews CASCADE;
\echo 'multihop_leaf_writes: PASS'

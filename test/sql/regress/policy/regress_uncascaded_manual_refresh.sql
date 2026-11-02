-- A table read only in an uncorrelated subquery cannot be cascaded: with
-- uncascaded_policy "warn" the TVIEW is created and its rows go stale on that
-- table's writes, and pg_tviews_refresh(<entity>) brings them back:
--
-- 1. an aggregated array of comments, each embedding its author two tables away
--    (tb_user -> tb_comment -> tv_post): the author's new name appears;
-- 2. an aggregated array of products: several prices changed in one statement
--    all appear.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/policy/regress_uncascaded_manual_refresh.sql
-- expect-output: uncascaded_manual_refresh: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'uncascaded_manual_refresh FAIL: %', what; END IF; END $$;

-- 1. Nested authors.
CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, email text);
CREATE TABLE tb_comment (pk_comment bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_user bigint REFERENCES tb_user, text text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO tb_user (pk_user, name, email) VALUES (1, 'Alice', 'alice@example.com');
INSERT INTO tb_comment (pk_comment, fk_user, text) VALUES (1, 1, 'Great post!');
INSERT INTO tb_post (pk_post, title) VALUES (1, 'Test Post');

SELECT pg_tviews_create('tv_post', $$
SELECT p.pk_post, p.id, jsonb_build_object('comments', COALESCE((
    SELECT jsonb_agg(jsonb_build_object('id', c.id, 'text', c.text,
                     'author', jsonb_build_object('id', u.id, 'name', u.name, 'email', u.email)))
    FROM tb_comment c JOIN tb_user u ON c.fk_user = u.pk_user
    WHERE c.fk_user IS NOT NULL), '[]'::jsonb)) AS data
FROM tb_post p
$$, '{"uncascaded_policy": "warn"}');

UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1;
SELECT pg_tviews_refresh('post');
SELECT must((SELECT data->'comments'->0->'author'->>'name' FROM tv_post WHERE pk_post = 1) = 'Alice Updated',
            'nested author name not refreshed by pg_tviews_refresh');
SELECT assert_fresh('tv_post', 'pk_post', 'pg_tviews_refresh(post)');

-- 2. Several prices in one statement.
CREATE TABLE tb_product (pk_product bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         name text, price numeric(10,2));
CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), customer_name text);
INSERT INTO tb_product (pk_product, name, price) VALUES (1, 'Laptop', 999.99), (2, 'Mouse', 29.99), (3, 'Keyboard', 79.99);
INSERT INTO tb_order (pk_order, customer_name) VALUES (1, 'John Doe');

SELECT pg_tviews_create('tv_order', $$
SELECT o.pk_order, o.id, jsonb_build_object('items', COALESCE((
    SELECT jsonb_agg(jsonb_build_object('id', p.id, 'name', p.name, 'price', p.price) ORDER BY p.pk_product)
    FROM tb_product p WHERE p.pk_product IN (1, 2, 3)), '[]'::jsonb)) AS data
FROM tb_order o
$$, '{"uncascaded_policy": "warn"}');

UPDATE tb_product SET price = price * 1.1 WHERE pk_product IN (1, 2);
SELECT pg_tviews_refresh('order');
SELECT must((SELECT (data->'items'->0->>'price')::numeric = 1099.99
                AND (data->'items'->1->>'price')::numeric = 32.99
                AND (data->'items'->2->>'price')::numeric = 79.99
             FROM tv_order WHERE pk_order = 1),
            'prices not refreshed by pg_tviews_refresh');
SELECT assert_fresh('tv_order', 'pk_order', 'pg_tviews_refresh(order)');

\echo 'uncascaded_manual_refresh: PASS'

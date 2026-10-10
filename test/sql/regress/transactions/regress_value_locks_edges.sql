-- Edge cases of value locks (ADR 0207).
--
-- 1. A writer and a refresh meet on the same lock when the joined values are
--    equal but their text forms differ (numeric 5 = 5.00, a case-insensitive
--    collation): values are locked by the hash of their type's equality, in the
--    written column's type and collation.
-- 2. Under REPEATABLE READ, the cross-check against the latest snapshot fails
--    only on a concurrent change, not on what a recompute itself makes of the
--    rows: a UNION view returning two rows for one key (union_duplicate_policy
--    'first' keeps one) refreshes without 40001.
--
-- A prepared transaction holds the writer's locks while this session runs the
-- other side, with lock_timeout to tell a wait from none.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_value_locks_edges.sql
-- expect-output: value_locks_edges: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'value_locks_edges FAIL: %', what; END IF; END $$;

-- 1. numeric 5 = numeric 5.00.
CREATE TABLE tb_price (code numeric PRIMARY KEY, label text);
CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      price_code numeric(10,2), name text);
INSERT INTO tb_price VALUES (5, 'five');
INSERT INTO tb_item VALUES (1, DEFAULT, 5, 'i1');
SELECT pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, jsonb_build_object('name', i.name, 'price', upper(p.label)) AS data
    FROM tb_item i JOIN tb_price p ON p.code = i.price_code $$);
BEGIN;
UPDATE tb_price SET label = 'FIVE!' WHERE code = 5;
PREPARE TRANSACTION 'value_locks_edges';
CREATE TEMP TABLE waited AS SELECT NULL::boolean AS ok;
DO $$
BEGIN
    BEGIN
        SET LOCAL lock_timeout = '200ms';
        INSERT INTO tb_item VALUES (2, DEFAULT, 5, 'i2');
        RAISE EXCEPTION 'no wait';
    EXCEPTION
        WHEN lock_not_available THEN UPDATE waited SET ok = true;
        WHEN raise_exception THEN UPDATE waited SET ok = false;
    END;
END $$;
COMMIT PREPARED 'value_locks_edges';
SELECT must((SELECT ok FROM waited), 'a refresh reading price 5.00 did not wait for the writer of price 5');
DROP TABLE waited;
INSERT INTO tb_item VALUES (2, DEFAULT, 5, 'i2');
SELECT assert_fresh('tv_item', 'pk_item', 'numeric 5 = 5.00');
SELECT must((SELECT column_name FROM tviews.pg_tviews_read_set_queries('tv_item', 'tb_price'::regclass))
            = 'code', 'the join on numerics is not locked by value');

-- ... and a case-insensitive collation: 'Ann' = 'ann'.
CREATE COLLATION ci (provider = icu, locale = 'und-u-ks-level2', deterministic = false);
CREATE TABLE tb_owner (login text COLLATE ci PRIMARY KEY, label text);
CREATE TABLE tb_doc (pk_doc bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     owner text COLLATE ci, name text);
INSERT INTO tb_owner VALUES ('Ann', 'a');
INSERT INTO tb_doc VALUES (1, DEFAULT, 'Ann', 'd1');
SELECT pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('name', d.name, 'owner', upper(o.label)) AS data
    FROM tb_doc d JOIN tb_owner o ON o.login = d.owner $$);
BEGIN;
UPDATE tb_owner SET label = 'b' WHERE login = 'Ann';
PREPARE TRANSACTION 'value_locks_edges';
CREATE TEMP TABLE waited AS SELECT NULL::boolean AS ok;
DO $$
BEGIN
    BEGIN
        SET LOCAL lock_timeout = '200ms';
        INSERT INTO tb_doc VALUES (2, DEFAULT, 'ann', 'd2');
        RAISE EXCEPTION 'no wait';
    EXCEPTION
        WHEN lock_not_available THEN UPDATE waited SET ok = true;
        WHEN raise_exception THEN UPDATE waited SET ok = false;
    END;
END $$;
COMMIT PREPARED 'value_locks_edges';
SELECT must((SELECT ok FROM waited), 'a refresh reading ''ann'' did not wait for the writer of ''Ann''');
DROP TABLE waited;
INSERT INTO tb_doc VALUES (2, DEFAULT, 'ann', 'd2');
SELECT assert_fresh('tv_doc', 'pk_doc', 'case-insensitive collation');

-- 2. A UNION view with a duplicated key, REPEATABLE READ (two keys in one
--    statement: the bulk refresh keeps one row per key).
CREATE TABLE tb_task (pk_task integer PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
CREATE TABLE tb_task_copy (pk_task integer PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO tb_task VALUES (1, DEFAULT, 'a'), (3, DEFAULT, 'c');
SELECT pg_tviews_create('tv_task', $TV$
  SELECT pk_task, id, jsonb_build_object('title', title, 'copy', false) AS data FROM tb_task
  UNION ALL
  SELECT pk_task, id, jsonb_build_object('title', title, 'copy', true) AS data FROM tb_task_copy
$TV$);
SET pg_tviews.union_duplicate_policy = 'first';
INSERT INTO tb_task_copy VALUES (1, DEFAULT, 'a'), (4, DEFAULT, 'd');
BEGIN ISOLATION LEVEL REPEATABLE READ;
UPDATE tb_task SET title = title || '!' WHERE pk_task IN (1, 3);
COMMIT;
SELECT must((SELECT count(*) FROM tv_task WHERE pk_task = 1) = 1, 'the duplicated key is not one row');
RESET pg_tviews.union_duplicate_policy;

\echo 'value_locks_edges: PASS'

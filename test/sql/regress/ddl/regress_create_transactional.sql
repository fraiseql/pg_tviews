-- TVIEW creation is transactional and both creation forms agree.
--
-- 1. CREATE TABLE tv_* AS and pg_tviews_create() over identical tables produce the
--    same rows and each a registration, a backing view and change triggers; a
--    TVIEW joining two tables is populated from the join.
-- 2. pg_tviews_drop() removes the table and its registration.
-- 3. A TVIEW created in a transaction that commits persists; one created in a
--    transaction that rolls back, or after a savepoint rolled back to, leaves no
--    table and no registration, while one created before the savepoint survives.
-- 4. A definition whose pk_<entity> repeats (a composite base key projected to
--    one of its columns) is refused, and the refusal leaves nothing behind.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_create_transactional.sql
-- expect-output: create_transactional: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'create_transactional FAIL: %', what; END IF; END $$;

-- True when entity `e` has neither a tv_<e> table nor a registration.
CREATE FUNCTION gone(e text) RETURNS boolean LANGUAGE sql AS $$
    SELECT to_regclass('tv_' || e) IS NULL
       AND NOT EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = e) $$;

-- 1. Both creation forms.
CREATE TABLE tb_ddl_user (pk_ddl_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                          name text NOT NULL, email text);
CREATE TABLE tb_fn_user (LIKE tb_ddl_user INCLUDING ALL);
ALTER TABLE tb_fn_user RENAME COLUMN pk_ddl_user TO pk_fn_user;
INSERT INTO tb_ddl_user (pk_ddl_user, name, email) VALUES (1, 'Alice', 'alice@example.com'), (2, 'Bob', 'bob@example.com');
INSERT INTO tb_fn_user (pk_fn_user, name, email) SELECT pk_ddl_user, name, email FROM tb_ddl_user;

CREATE TABLE tv_ddl_user AS
SELECT pk_ddl_user, id, jsonb_build_object('name', name, 'email', email) AS data FROM tb_ddl_user;
SELECT pg_tviews_create('fn_user', $$
SELECT pk_fn_user, id, jsonb_build_object('name', name, 'email', email) AS data FROM tb_fn_user $$);

SELECT must((SELECT count(*) FROM tv_ddl_user) = 2 AND (SELECT count(*) FROM tv_fn_user) = 2,
            'each form populates 2 rows');
SELECT must(NOT EXISTS (SELECT pk_ddl_user, data FROM tv_ddl_user
                        EXCEPT SELECT pk_fn_user, data FROM tv_fn_user),
            'CTAS and pg_tviews_create() produce different documents');
SELECT must((SELECT count(*) FROM tviews.pg_tview_meta WHERE entity IN ('ddl_user', 'fn_user')) = 2,
            'both forms registered');

CREATE TABLE tb_user_c (pk_user_c bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE, name text);
CREATE TABLE tb_post_c (pk_post_c bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                        fk_user_c bigint NOT NULL REFERENCES tb_user_c, title text);
INSERT INTO tb_user_c (pk_user_c, name) VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Charlie');
INSERT INTO tb_post_c (pk_post_c, fk_user_c, title) VALUES (1, 1, 'a1'), (2, 1, 'a2'), (3, 2, 'b1'), (4, 3, 'c1');
CREATE TABLE tv_user_c AS
SELECT pk_user_c, id, jsonb_build_object('name', name) AS data FROM tb_user_c;
CREATE TABLE tv_post_c AS
SELECT p.pk_post_c, p.id, jsonb_build_object('title', p.title, 'author_id', u.id) AS data
FROM tb_post_c p JOIN tb_user_c u ON p.fk_user_c = u.pk_user_c;
SELECT must((SELECT count(*) FROM tv_user_c) = 3 AND (SELECT count(*) FROM tv_post_c) = 4,
            'sequential CTAS TVIEWs populated (3 users, 4 posts)');
SELECT assert_fresh('tv_post_c', 'pk_post_c', 'CTAS over a join');

SELECT must((SELECT count(*) FROM pg_views WHERE schemaname = 'tviews'
             AND viewname IN ('public__tv_user_c', 'public__tv_post_c', 'public__tv_ddl_user',
                              'public__tv_fn_user')) = 4, 'backing views missing');
SELECT must((SELECT count(DISTINCT tgrelid) FROM pg_trigger WHERE tgname LIKE 'trg_tview_%'
             AND tgrelid IN ('tb_user_c'::regclass, 'tb_post_c'::regclass, 'tb_ddl_user'::regclass,
                             'tb_fn_user'::regclass)) = 4, 'change triggers missing on a base table');

-- 2. pg_tviews_drop().
SELECT pg_tviews_drop('tv_ddl_user', true, true);
SELECT must(gone('ddl_user'), 'pg_tviews_drop left tv_ddl_user or its registration');

-- 3. Transactions and savepoints.
BEGIN;
CREATE TABLE tb_txn (pk_txn bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE, value text);
INSERT INTO tb_txn VALUES (1, DEFAULT, 'test1'), (2, DEFAULT, 'test2');
CREATE TABLE tv_txn AS SELECT pk_txn, id, jsonb_build_object('value', value) AS data FROM tb_txn;
COMMIT;
SELECT must((SELECT count(*) FROM tv_txn) = 2, 'TVIEW created in a committed transaction lost its rows');
SELECT must(EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'txn'), 'committed TVIEW not registered');

BEGIN;
CREATE TABLE tb_rb (pk_rb bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE, value text);
INSERT INTO tb_rb VALUES (1, DEFAULT, 'x'), (2, DEFAULT, 'y');
CREATE TABLE tv_rb AS SELECT pk_rb, id, jsonb_build_object('value', value) AS data FROM tb_rb;
SELECT must((SELECT count(*) FROM tv_rb) = 2, 'TVIEW not populated inside its transaction');
ROLLBACK;
SELECT must(gone('rb'), 'a rolled-back TVIEW left its table or registration');

BEGIN;
CREATE TABLE tb_sp_a (pk_sp_a bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE, v text);
INSERT INTO tb_sp_a VALUES (1, DEFAULT, 'a1'), (2, DEFAULT, 'a2');
CREATE TABLE tv_sp_a AS SELECT pk_sp_a, id, jsonb_build_object('v', v) AS data FROM tb_sp_a;
SAVEPOINT sp1;
CREATE TABLE tb_sp_b (pk_sp_b bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE, v text);
INSERT INTO tb_sp_b VALUES (1, DEFAULT, 'b1');
CREATE TABLE tv_sp_b AS SELECT pk_sp_b, id, jsonb_build_object('v', v) AS data FROM tb_sp_b;
SELECT must((SELECT count(*) FROM tv_sp_b) = 1, 'tv_sp_b not created');
ROLLBACK TO sp1;
SELECT must((SELECT count(*) FROM tv_sp_a) = 2, 'tv_sp_a lost to ROLLBACK TO a later savepoint');
SELECT must(gone('sp_b'), 'tv_sp_b survived ROLLBACK TO sp1');
COMMIT;
SELECT must((SELECT count(*) FROM tv_sp_a) = 2 AND EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'sp_a'),
            'tv_sp_a not committed');

-- 4. A key that repeats is refused, leaving nothing.
CREATE TABLE tb_composite (pk_composite_1 int, pk_composite_2 int, id uuid NOT NULL DEFAULT gen_random_uuid(),
                           payload text, PRIMARY KEY (pk_composite_1, pk_composite_2));
INSERT INTO tb_composite (pk_composite_1, pk_composite_2, payload) VALUES (1, 1, 'x'), (1, 2, 'y');
DO $$
BEGIN
    PERFORM pg_tviews_create('tv_composite', $q$
        SELECT pk_composite_1 AS pk_composite, id, jsonb_build_object('payload', payload) AS data
        FROM tb_composite $q$);
    RAISE EXCEPTION 'create_transactional FAIL: a TVIEW whose key repeats was accepted';
EXCEPTION WHEN OTHERS THEN
    IF SQLERRM LIKE 'create_transactional FAIL%' THEN RAISE; END IF;
END $$;
SELECT must(gone('composite'), 'a refused TVIEW left its table or registration');

\echo 'create_transactional: PASS'

-- Regression test (#233): pg_tviews.max_refresh_rows caps the TVIEW rows one
-- statement's refresh writes.
--
-- A write to a column embedded in many rows refreshes all of them in the writer's
-- statement. With the guard on, a statement whose refresh would write more rows
-- than the limit fails with 54000 before writing them, and the write rolls back
-- with it. The limit is per statement (one flush), charged before each batch:
-- recomputed keys, direct patches, fan-out children (counted through the lookup
-- index), a full refresh's rows. Explicit rebuilds are not charged: they are the
-- remedy (ADR 0233).
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_max_refresh_rows.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'max_refresh_rows FAIL: %', what; END IF; END $$;

-- The SQLSTATE and message `stmt` fails with, NULL when it succeeds.
CREATE FUNCTION failure(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ' ' || SQLERRM; END $$;

-- Category 1 embedded in 1,000 items, categories 2 to 11 in 50 each.
CREATE TABLE tb_cat (pk_cat int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     name text NOT NULL);
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_cat int NOT NULL REFERENCES tb_cat, title text NOT NULL);
CREATE INDEX ON tb_item (fk_cat);
INSERT INTO tb_cat (pk_cat, name) SELECT g, 'cat ' || g FROM generate_series(1, 11) g;
INSERT INTO tb_item (pk_item, fk_cat, title) SELECT g, 1, 't' FROM generate_series(1, 1000) g;
INSERT INTO tb_item (pk_item, fk_cat, title)
    SELECT 1000 + (c - 2) * 50 + i, c, 't' FROM generate_series(2, 11) c, generate_series(1, 50) i;
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, i.fk_cat,
           jsonb_build_object('title', i.title, 'category', jsonb_build_object('name', c.name)) AS data
    FROM tb_item i JOIN tb_cat c ON c.pk_cat = i.fk_cat $$);
ANALYZE tb_cat, tb_item, tv_item;

-- 1. Off by default: a 1,000-row fan-out succeeds.
SELECT must(current_setting('pg_tviews.max_refresh_rows') = '0', 'the default is not 0');
UPDATE tb_cat SET name = 'one' WHERE pk_cat = 1;
SELECT must((SELECT count(*) FROM tv_item WHERE data #>> '{category,name}' = 'one') = 1000,
            'the 1,000 children were not patched');

-- 2. Limit 100: the 1,000-row fan-out fails before writing, and names what it would do.
SET pg_tviews.max_refresh_rows = 100;
SELECT failure($$UPDATE tb_cat SET name = 'two' WHERE pk_cat = 1$$) AS err \gset
SELECT must(:'err' LIKE '54000 %', 'not 54000: ' || coalesce(:'err', 'it succeeded'));
SELECT must(:'err' LIKE '%tv_item%' AND :'err' LIKE '%fk_cat%' AND :'err' LIKE '%1000 rows%'
            AND :'err' LIKE '%max_refresh_rows = 100%', 'message: ' || :'err');
SELECT must((SELECT name FROM tb_cat WHERE pk_cat = 1) = 'one', 'the base write was kept');
SELECT must((SELECT count(*) FROM tv_item WHERE data #>> '{category,name}' = 'one') = 1000,
            'children were written');

-- 3. Per statement: ten 50-row statements in one transaction succeed.
BEGIN;
UPDATE tb_cat SET name = 'many' WHERE pk_cat = 2;
UPDATE tb_cat SET name = 'many' WHERE pk_cat = 3;
UPDATE tb_cat SET name = 'many' WHERE pk_cat = 4;
UPDATE tb_cat SET name = 'many' WHERE pk_cat = 5;
UPDATE tb_cat SET name = 'many' WHERE pk_cat = 6;
UPDATE tb_cat SET name = 'many' WHERE pk_cat = 7;
UPDATE tb_cat SET name = 'many' WHERE pk_cat = 8;
UPDATE tb_cat SET name = 'many' WHERE pk_cat = 9;
UPDATE tb_cat SET name = 'many' WHERE pk_cat = 10;
UPDATE tb_cat SET name = 'many' WHERE pk_cat = 11;
COMMIT;
SELECT must((SELECT count(*) FROM tv_item WHERE data #>> '{category,name}' = 'many') = 500,
            'the small statements were refused');
-- Rows of one statement are charged together: 100 fit, 101 don't.
SELECT must(failure($$UPDATE tb_cat SET name = 'pair' WHERE pk_cat IN (2, 3)$$) IS NULL,
            'a 100-row statement was refused');
SELECT must(failure($$UPDATE tb_item SET title = 'u' WHERE pk_item BETWEEN 1 AND 101$$) LIKE '54000 %',
            'a 101-row patch was accepted');

-- 4. The recompute path is charged the same way.
SET pg_tviews.direct_patch_enabled = off;
SELECT failure($$UPDATE tb_cat SET name = 'three' WHERE pk_cat = 1$$) AS err \gset
SELECT must(:'err' LIKE '54000 %' AND :'err' LIKE '%tv_item%' AND :'err' LIKE '%1000 rows%',
            'recompute: ' || coalesce(:'err', 'it succeeded'));
RESET pg_tviews.direct_patch_enabled;

-- 5. A full refresh is charged with the TVIEW's rows.
CREATE TABLE tb_flag (pk_flag int PRIMARY KEY, on_off boolean NOT NULL);
INSERT INTO tb_flag VALUES (1, true);
SELECT tviews.pg_tviews_create_or_replace('tv_flagged', $$
    SELECT pk_item AS pk_flagged, id,
           jsonb_build_object('flags', (SELECT count(*) FROM tb_flag WHERE on_off)) AS data
    FROM tb_item $$, '{"uncascaded_policy": "full_refresh"}');
SELECT failure($$INSERT INTO tb_flag VALUES (2, true)$$) AS err \gset
SELECT must(:'err' LIKE '54000 %' AND :'err' LIKE '%tv_flagged%' AND :'err' LIKE '%full refresh%',
            'full refresh: ' || coalesce(:'err', 'it succeeded'));
RESET pg_tviews.max_refresh_rows;
INSERT INTO tb_flag VALUES (2, true);
SELECT assert_fresh('tv_flagged', 'pk_flagged', 'the full refresh without a limit');
SET pg_tviews.max_refresh_rows = 100;

-- 6. The remedy: a suspended write is rebuilt once at resume, which is not charged.
BEGIN;
SELECT tviews.pg_tviews_suspend_triggers();
UPDATE tb_cat SET name = 'four' WHERE pk_cat = 1;
SELECT tviews.pg_tviews_resume_triggers();
COMMIT;
SELECT must((SELECT count(*) FROM tv_item WHERE data #>> '{category,name}' = 'four') = 1000,
            'the suspended write was not refreshed');
SELECT assert_fresh('tv_item', 'pk_item', 'a suspended write after its resume');

-- 7. Only a superuser sets it.
RESET pg_tviews.max_refresh_rows;
CREATE ROLE max_refresh_rows_user;
SET ROLE max_refresh_rows_user;
SELECT failure($$SET pg_tviews.max_refresh_rows = 5$$) AS err \gset
RESET ROLE;
SELECT must(:'err' LIKE '42501 %', 'a non-superuser set it: ' || coalesce(:'err', 'it succeeded'));
DROP ROLE max_refresh_rows_user;

-- 8. The queue limit keeps its SQLSTATE and message.
SET pg_tviews.max_queue_size = 10;
SELECT failure($$UPDATE tb_item SET title = 'q' WHERE pk_item BETWEEN 1 AND 50$$) AS err \gset
SELECT must(:'err' LIKE '54000 %' AND :'err' LIKE '%queue size%max_queue_size (10)%',
            'queue limit: ' || coalesce(:'err', 'it succeeded'));
RESET pg_tviews.max_queue_size;

\echo 'max_refresh_rows (#233): PASS'

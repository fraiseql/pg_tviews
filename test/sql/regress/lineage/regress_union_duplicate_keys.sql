-- A UNION TVIEW whose branches return one key twice (#216, ADR 0216):
--   1. every path refuses it with 21000 and the same hint: creation, a write
--      refreshing one key, a write refreshing several, pg_tviews_refresh(),
--      pg_tviews_refresh_all(); never a raw duplicate-key error;
--   2. pg_tviews.union_duplicate_policy no longer exists;
--   3. keeping one row per key is written in the definition: DISTINCT ON over the
--      UNION, ordered by preference, is a TVIEW whose key stands for a column of
--      each branch, so a write to either table refreshes it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_union_duplicate_keys.sql
-- expect-output: union_duplicate_keys: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'union_duplicate_keys FAIL: %', what; END IF; END $$;
-- The SQLSTATE and the message of a statement's error, or 'ok'.
CREATE FUNCTION outcome(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'ok';
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ' ' || SQLERRM; END $$;
CREATE FUNCTION refused(stmt text, what text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE o text := outcome(stmt);
BEGIN
    IF o NOT LIKE '21000 %multiple rows for pk_task=1%' THEN
        RAISE EXCEPTION 'union_duplicate_keys FAIL: %: %', what, o;
    END IF;
END $$;

CREATE TABLE tb_task (pk_task integer PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
CREATE TABLE tb_task_copy (pk_task integer PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO tb_task VALUES (1, DEFAULT, 'a'), (2, DEFAULT, 'b');
\set union 'SELECT pk_task, id, jsonb_build_object(''title'', title, ''copy'', false) AS data FROM tb_task UNION ALL SELECT pk_task, id, jsonb_build_object(''title'', title, ''copy'', true) AS data FROM tb_task_copy'

-- 1. Refused on every path.
INSERT INTO tb_task_copy VALUES (1, DEFAULT, 'a');
SELECT refused(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_task', :'union'), 'create');
DELETE FROM tb_task_copy;
SELECT tviews.pg_tviews_create('tv_task', :'union');
SELECT refused($$INSERT INTO tb_task_copy VALUES (1, DEFAULT, 'a')$$, 'a write refreshing one key');
SELECT refused($$INSERT INTO tb_task_copy VALUES (1, DEFAULT, 'a'), (2, DEFAULT, 'b')$$,
               'a write refreshing several keys');
-- The duplicate written behind the triggers' back.
ALTER TABLE tb_task_copy DISABLE TRIGGER USER;
INSERT INTO tb_task_copy VALUES (1, DEFAULT, 'a');
ALTER TABLE tb_task_copy ENABLE TRIGGER USER;
SELECT refused($$UPDATE tb_task SET title = 'a!' WHERE pk_task = 1$$, 'a write to the other branch');
SELECT refused($$SELECT tviews.pg_tviews_refresh('task')$$, 'pg_tviews_refresh');
SELECT refused($$SELECT tviews.pg_tviews_refresh_all()$$, 'pg_tviews_refresh_all');
SELECT must(outcome($$SET pg_tviews.union_duplicate_policy = 'first'$$) <> 'ok',
            'pg_tviews.union_duplicate_policy still exists');
-- The hint names both ways out.
DO $$
DECLARE hint text;
BEGIN
    PERFORM tviews.pg_tviews_refresh('task');
EXCEPTION WHEN cardinality_violation THEN
    GET STACKED DIAGNOSTICS hint = PG_EXCEPTION_HINT;
    PERFORM must(hint LIKE '%disjoint%' AND hint LIKE '%DISTINCT ON%', 'hint: ' || hint);
END $$;
DELETE FROM tb_task_copy;
SELECT tviews.pg_tviews_drop('tv_task');

-- 3. One row per key, written in the definition: the first branch is preferred.
INSERT INTO tb_task_copy VALUES (1, DEFAULT, 'a copy'), (3, DEFAULT, 'only a copy');
SELECT tviews.pg_tviews_create('tv_task', $TV$
  SELECT DISTINCT ON (u.pk_task) u.pk_task, u.id, u.data
  FROM (SELECT pk_task, id, jsonb_build_object('title', title, 'copy', false) AS data, 1 AS pref FROM tb_task
        UNION ALL
        SELECT pk_task, id, jsonb_build_object('title', title, 'copy', true) AS data, 2 AS pref FROM tb_task_copy) u
  ORDER BY u.pk_task, u.pref $TV$);
SELECT must(identity = '{pk_task}' AND uncascaded_tables = '{}',
            'identity ' || identity::text || ', uncascaded ' || uncascaded_tables::text)
FROM tviews.registry WHERE entity = 'task';
SELECT must((SELECT data->>'copy' FROM tv_task WHERE pk_task = 1) = 'false', 'the preferred row was not kept');
SELECT assert_fresh('tv_task', 'pk_task', 'created');
UPDATE tb_task SET title = 'a!' WHERE pk_task = 1;
SELECT assert_fresh('tv_task', 'pk_task', 'a write to the preferred branch');
UPDATE tb_task_copy SET title = 'c!' WHERE pk_task = 3;
SELECT assert_fresh('tv_task', 'pk_task', 'a write to the other branch');
DELETE FROM tb_task WHERE pk_task = 1;
SELECT must((SELECT data->>'copy' FROM tv_task WHERE pk_task = 1) = 'true',
            'the other branch''s row did not take over');
SELECT assert_fresh('tv_task', 'pk_task', 'the preferred row deleted');
INSERT INTO tb_task VALUES (3, DEFAULT, 'now preferred');
SELECT must((SELECT data->>'copy' FROM tv_task WHERE pk_task = 3) = 'false',
            'an inserted preferred row did not take over');
INSERT INTO tb_task_copy VALUES (2, DEFAULT, 'b copy'), (4, DEFAULT, 'd copy');
SELECT assert_fresh('tv_task', 'pk_task', 'several keys in one write');
SELECT tviews.pg_tviews_refresh('task');
SELECT assert_fresh('tv_task', 'pk_task', 'pg_tviews_refresh');

\echo 'union_duplicate_keys: PASS'

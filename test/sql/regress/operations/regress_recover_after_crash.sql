-- pg_tviews_recover_after_crash(entity) rebuilds a TVIEW only when it looks
-- crash-reset: its table is empty while its backing view has rows.
--
-- A crash empties every UNLOGGED table. The function is safe to call on every
-- TVIEW after a restart: it returns true when it rebuilt the TVIEW, false when
-- nothing was needed (rows present, or a view that is empty too), and fails on a
-- name that is not a TVIEW. pg_tviews_replication_status() reports the same
-- condition as needs_rebuild. TRUNCATE stands in for the crash.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_recover_after_crash.sql
-- expect-output: recover_after_crash: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'recover_after_crash FAIL: %', what; END IF; END $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'alice'), (2, 'bob');
SELECT pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('name', name) AS data FROM tb_item $$);

CREATE TABLE tb_empty (pk_empty int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
SELECT pg_tviews_create('tv_empty', $$
    SELECT pk_empty, id, jsonb_build_object('name', name) AS data FROM tb_empty $$);

-- 1. A populated TVIEW needs nothing.
SELECT must(NOT pg_tviews_recover_after_crash('item'), 'recovered a populated TVIEW');
SELECT must((SELECT NOT needs_rebuild FROM pg_tviews_replication_status() WHERE entity = 'item'),
            'a populated TVIEW reported as needing a rebuild');

-- 2. Emptied (as a crash empties an UNLOGGED table): reported, then rebuilt.
TRUNCATE tv_item;
SELECT must((SELECT needs_rebuild FROM pg_tviews_replication_status() WHERE entity = 'item'),
            'an emptied TVIEW whose view has rows not reported as needing a rebuild');
SELECT must((SELECT count(*) FROM tviews.public__tv_item) = 2, 'the backing view lost its rows');
SELECT must(pg_tviews_recover_after_crash('item'), 'an emptied TVIEW was not recovered');
SELECT must((SELECT count(*) FROM tv_item) = 2, 'recovery did not repopulate tv_item');
SELECT assert_fresh('tv_item', 'pk_item', 'recover_after_crash');

-- 3. Calling it again is a no-op.
SELECT must(NOT pg_tviews_recover_after_crash('item'), 'a second recovery rebuilt again');

-- 4. Incremental refresh works after the recovery.
UPDATE tb_item SET name = 'carol' WHERE pk_item = 2;
SELECT assert_fresh('tv_item', 'pk_item', 'an UPDATE after the recovery');

-- 5. An empty TVIEW over an empty view is not a crash.
SELECT must(NOT pg_tviews_recover_after_crash('empty'), 'recovered a TVIEW whose view is empty');

-- 6. An unknown entity is an error, not false.
DO $$
BEGIN
    PERFORM pg_tviews_recover_after_crash('nosuch');
    RAISE EXCEPTION 'recover_after_crash FAIL: an unknown entity did not fail';
EXCEPTION WHEN OTHERS THEN
    IF SQLERRM LIKE 'recover_after_crash FAIL%' THEN RAISE; END IF;
    IF SQLERRM NOT LIKE '%nosuch%' THEN
        RAISE EXCEPTION 'recover_after_crash FAIL: the error does not name the entity: %', SQLERRM;
    END IF;
END $$;

\echo 'recover_after_crash: PASS'

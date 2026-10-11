-- Regression test (#152): pg_tviews_drop(if_exists => true) on a missing TVIEW.
--
-- It returned "dropped successfully" when there was nothing to drop. Like
-- DROP TABLE IF EXISTS, it now raises a NOTICE that the TVIEW does not exist and
-- returns text saying nothing was dropped. The NOTICE is read from a second psql's
-- output (a NOTICE cannot be caught in SQL).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_drop_if_exists.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#152 FAIL: %', what; END IF; END $$;

-- 1. A missing TVIEW with if_exists: a NOTICE, and a result that says nothing was
--    dropped.
\set missing `psql -X -At -h :HOST -p :PORT -U :USER -d :DBNAME -c "SET client_min_messages = notice" -c "SELECT tviews.pg_tviews_drop('tv_nosuch', if_exists => true)" 2>&1`
SELECT must(:'missing' LIKE '%NOTICE:  TVIEW "tv_nosuch" does not exist, skipping%',
            'no NOTICE: ' || :'missing');
SELECT must(:'missing' LIKE '%TVIEW tv_nosuch does not exist, nothing dropped%',
            'result does not say nothing was dropped: ' || :'missing');
SELECT must(:'missing' NOT LIKE '%dropped successfully%',
            'a missing TVIEW is reported as dropped: ' || :'missing');

-- 2. Without if_exists it is still an error.
DO $$
BEGIN
    PERFORM tviews.pg_tviews_drop('tv_nosuch');
    RAISE EXCEPTION '#152 FAIL: dropping a missing TVIEW without if_exists did not fail';
EXCEPTION WHEN others THEN
    IF SQLERRM LIKE '#152 FAIL%' THEN RAISE; END IF;
END $$;

-- 3. An existing TVIEW is dropped, without the NOTICE.
\set existing `psql -X -At -h :HOST -p :PORT -U :USER -d :DBNAME -c "SET client_min_messages = notice" -c "SELECT tviews.pg_tviews_drop('tv_user', if_exists => true)" 2>&1`
SELECT must(:'existing' LIKE '%TVIEW public.tv_user dropped%',
            'existing TVIEW: ' || :'existing');
SELECT must(:'existing' NOT LIKE '%does not exist%', 'existing TVIEW: ' || :'existing');
SELECT must(to_regclass('tv_user') IS NULL, 'tv_user still exists');

DROP EXTENSION pg_tviews CASCADE;

SELECT 'issue #152 drop if_exists: PASS' AS result;
-- expect-output: issue #152 drop if_exists: PASS

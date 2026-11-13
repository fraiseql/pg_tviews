-- A TVIEW whose data is the whole row of the table holding its key refreshes on
-- an UPDATE of any column of that table: the whole row reads every column.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_wholerow_root.sql
--
-- expect-output: whole-row root read: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

-- The refreshes forced because the definition reads a table whole, by reason
-- (test-hooks builds; NULL in others).
CREATE FUNCTION stopgap(reason text) RETURNS bigint LANGUAGE sql AS $$
    SELECT CASE WHEN tviews.pg_tviews_show_queue() ? 'patch_stats'
                THEN coalesce((tviews.pg_tviews_show_queue()->'patch_stats'->>('stopgap:' || reason))::bigint, 0)
           END $$;
CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;

CREATE TABLE tb_acct (pk_acct int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, is_disabled boolean NOT NULL DEFAULT false);
INSERT INTO tb_acct (pk_acct, name) VALUES (1, 'a'), (2, 'b');
SELECT tviews.pg_tviews_create('tv_acct', $$
    SELECT a.pk_acct, a.id, to_jsonb(a) AS data FROM tb_acct a $$) IS NOT NULL AS created;

-- The same read through a view of the chain.
CREATE VIEW v_acct_row AS SELECT a.pk_acct, a.id, to_jsonb(a) AS raw FROM tb_acct a;
SELECT tviews.pg_tviews_create('tv_acct_via', $$
    SELECT v.pk_acct AS pk_acct_via, v.id, jsonb_build_object('raw', v.raw) AS data FROM v_acct_row v $$) IS NOT NULL AS created;

UPDATE tb_acct SET is_disabled = true WHERE pk_acct = 1;
SELECT assert_fresh('tv_acct', 'pk_acct', 'an UPDATE of a column read only through the whole row');
SELECT assert_fresh('tv_acct_via', 'pk_acct_via', 'the same UPDATE, read whole in a view of the chain');
SELECT must((SELECT (data->>'is_disabled')::boolean FROM tv_acct WHERE pk_acct = 1), 'tv_acct row 1');

-- A column also read by name still refreshes the whole row.
UPDATE tb_acct SET name = 'b2', is_disabled = true WHERE pk_acct = 2;
SELECT assert_fresh('tv_acct', 'pk_acct', 'an UPDATE of a named and a whole-row column');

SELECT must(stopgap('wholerow') IS NULL OR stopgap('wholerow') > 0, 'no stopgap:wholerow counted');

SELECT 'whole-row root read: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;

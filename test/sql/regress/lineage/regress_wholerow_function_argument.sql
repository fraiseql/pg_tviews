-- A row passed whole to a function is read whole: the TVIEW refreshes on an UPDATE
-- of any column of it.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_wholerow_function_argument.sql
--
-- expect-output: whole row passed to a function: PASS

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

CREATE TABLE tb_f (pk_f int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), a text, b text);
INSERT INTO tb_f (pk_f, a, b) VALUES (1, 'a', 'b');
CREATE FUNCTION f_label(r tb_f) RETURNS text IMMUTABLE LANGUAGE sql AS $$ SELECT r.a || '/' || r.b $$;
SELECT tviews.pg_tviews_create('tv_f', $$
    SELECT f.pk_f, f.id, jsonb_build_object('label', f_label(f)) AS data FROM tb_f f $$) IS NOT NULL AS created;

UPDATE tb_f SET b = 'B' WHERE pk_f = 1;
SELECT assert_fresh('tv_f', 'pk_f', 'an UPDATE of a column the function reads off the whole row');

SELECT must(stopgap('wholerow') IS NULL OR stopgap('wholerow') > 0, 'no stopgap:wholerow counted');

SELECT 'whole row passed to a function: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
DROP FUNCTION f_label(tb_f);
DROP TABLE tb_f;

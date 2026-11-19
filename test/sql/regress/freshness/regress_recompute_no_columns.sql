-- A refresh that finds no column to compute (a catalog row naming a view that
-- has none) fails the write instead of committing it with the TVIEW untouched.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_recompute_no_columns.sql
-- expect-output: recompute without columns: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_a (pk_a int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_a (pk_a, name) VALUES (1, 'x'), (2, 'y');
-- upper() is not a copy: the write is recomputed, not patched.
SELECT tviews.pg_tviews_create('tv_a', $$
    SELECT a.pk_a, a.id, jsonb_build_object('name', upper(a.name)) AS data FROM tb_a a $$) IS NOT NULL AS created;

-- The catalog row now names a view with no column.
CREATE VIEW v_none AS SELECT FROM tb_a;
UPDATE tviews.pg_tview_meta SET view_oid = 'public.v_none'::regclass WHERE entity = 'a';

DO $$
BEGIN
    BEGIN
        UPDATE tb_a SET name = 'z' WHERE pk_a = 1;
    EXCEPTION WHEN OTHERS THEN
        RETURN;
    END;
    RAISE EXCEPTION 'FAIL: the write committed with nothing recomputed; tv_a holds %',
        (SELECT data FROM tv_a WHERE pk_a = 1);
END $$;

SELECT 'recompute without columns: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;

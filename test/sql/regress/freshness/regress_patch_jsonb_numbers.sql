-- A jsonb column copied into the TVIEW's own document (a direct patch) keeps its
-- numbers as written: a decimal with 22 digits and an integer past 2^64 are not
-- rounded to doubles.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_patch_jsonb_numbers.sql
-- expect-output: patched jsonb numbers: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_j (pk_j int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), meta jsonb, note text);
INSERT INTO tb_j (pk_j, meta, note) VALUES (1, '{"a": 1}', 'n');
SELECT tviews.pg_tviews_create('tv_j', $$
    SELECT j.pk_j, j.id, jsonb_build_object('meta', j.meta, 'note', j.note) AS data FROM tb_j j $$)
    IS NOT NULL AS created;

UPDATE tb_j SET meta = '{"a": 0.1000000000000000000001, "b": 12345678901234567890123}' WHERE pk_j = 1;
SELECT assert_fresh('tv_j', 'pk_j', 'an UPDATE of a jsonb column holding long numbers, patched');

SELECT 'patched jsonb numbers: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;

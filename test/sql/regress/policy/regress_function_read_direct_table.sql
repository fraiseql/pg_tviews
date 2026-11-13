-- A table read both directly and inside a declared function: an UPDATE of a column
-- only the function reads still refreshes the TVIEW as the table's policy says
-- (full_refresh here). The columns the definition names are not all it reads.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/policy/regress_function_read_direct_table.sql
--
-- expect-output: function read of a directly read table: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;
CREATE FUNCTION stopgap(reason text) RETURNS bigint LANGUAGE sql AS $$
    SELECT CASE WHEN tviews.pg_tviews_show_queue() ? 'patch_stats'
                THEN coalesce((tviews.pg_tviews_show_queue()->'patch_stats'->>('stopgap:' || reason))::bigint, 0)
           END $$;

CREATE TABLE tb_set (pk_set int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     name text, suffix text);
INSERT INTO tb_set (pk_set, name, suffix) VALUES (1, 'n', 's1');
CREATE FUNCTION sfx() RETURNS text STABLE LANGUAGE sql
    AS $$ SELECT suffix FROM public.tb_set WHERE pk_set = 1 $$;
CREATE TABLE tb_thing (pk_thing int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_set int NOT NULL REFERENCES tb_set, label text);
INSERT INTO tb_thing (pk_thing, fk_set, label) VALUES (1, 1, 'a'), (2, 1, 'b');

-- Read directly in a scalar subquery and inside sfx().
SELECT tviews.pg_tviews_create('tv_thing', $$
    SELECT t.pk_thing, t.id, jsonb_build_object('label', t.label || sfx(),
        'n', (SELECT s.name FROM tb_set s WHERE s.pk_set = 1)) AS data
    FROM tb_thing t $$,
    '{"function_reads": {"public.sfx()": ["public.tb_set"]}, "uncascaded_policy": "full_refresh"}')
    IS NOT NULL AS created;
-- Read directly by a join and inside sfx().
SELECT tviews.pg_tviews_create('tv_thing_join', $$
    SELECT t.pk_thing AS pk_thing_join, t.id, jsonb_build_object('label', t.label || sfx(), 'n', s.name) AS data
    FROM tb_thing t JOIN tb_set s ON s.pk_set = t.fk_set $$,
    '{"function_reads": {"public.sfx()": ["public.tb_set"]}, "uncascaded_policy": "full_refresh"}')
    IS NOT NULL AS created;

UPDATE tb_set SET suffix = 's2' WHERE pk_set = 1;
SELECT assert_fresh('tv_thing', 'pk_thing', 'an UPDATE of a column only the declared function reads');
SELECT assert_fresh('tv_thing_join', 'pk_thing_join', 'the same UPDATE, the table also joined');
UPDATE tb_set SET name = 'n2' WHERE pk_set = 1;
SELECT assert_fresh('tv_thing', 'pk_thing', 'an UPDATE of a column read directly');
SELECT assert_fresh('tv_thing_join', 'pk_thing_join', 'an UPDATE of a column read directly, joined');

SELECT must(stopgap('fnread') IS NULL OR stopgap('fnread') > 0, 'no stopgap:fnread counted');

SELECT 'function read of a directly read table: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;
DROP FUNCTION sfx();
DROP TABLE tb_thing, tb_set;

-- A TVIEW created from a raw SELECT without pk_<entity> / id columns gets its
-- id from pg_tviews: a row keeps it across refreshes, so a reader can hold on to
-- it. Its view must yield the same id on every read, as the table holds it.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_raw_select_stable_id.sql
--
-- known-failing: #321 (E1-14)
-- expect-output: raw select stable id: PASS

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

CREATE TABLE tb_tag (pk int PRIMARY KEY, label text);
INSERT INTO tb_tag VALUES (1, 'a'), (2, 'b');
SELECT tviews.pg_tviews_create('tv_tag', $$ SELECT pk, label FROM tb_tag $$) IS NOT NULL AS created;
CREATE TEMP TABLE ids AS SELECT pk_tag, id FROM tv_tag;

UPDATE tb_tag SET label = 'a2' WHERE pk = 1;
SELECT must((SELECT t.id = i.id FROM tv_tag t JOIN ids i USING (pk_tag) WHERE pk_tag = 1),
            'the written row kept its id');
SELECT must((SELECT t.id = i.id FROM tv_tag t JOIN ids i USING (pk_tag) WHERE pk_tag = 2),
            'the other row kept its id');
SELECT tviews.pg_tviews_refresh('tv_tag');
SELECT must((SELECT bool_and(t.id = i.id) FROM tv_tag t JOIN ids i USING (pk_tag)),
            'every row kept its id across a full refresh');
SELECT assert_fresh('tv_tag', 'pk_tag', 'an UPDATE and a full refresh');

SELECT 'raw select stable id: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;

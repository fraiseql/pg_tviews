-- A TVIEW created from a raw SELECT without pk_<entity> / id columns gets its
-- id from pg_tviews: a row keeps it across refreshes, so a reader can hold on to
-- it. Its view must yield the same id on every read, as the table holds it.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_raw_select_stable_id.sql
--
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

-- A source with its own uuid id keeps it; another TVIEW with the same keys gets
-- other ids (the entity is part of the name).
CREATE TABLE tb_doc (pk int PRIMARY KEY, id uuid NOT NULL, title text);
INSERT INTO tb_doc VALUES (1, '00000000-0000-4000-8000-000000000001', 't');
SELECT tviews.pg_tviews_create('tv_doc', $$ SELECT pk, id, title FROM tb_doc $$) IS NOT NULL AS created;
SELECT must((SELECT id = '00000000-0000-4000-8000-000000000001' FROM tv_doc), 'the source id is kept');
CREATE TABLE tb_twin (pk int PRIMARY KEY, label text);
INSERT INTO tb_twin VALUES (1, 'a');
SELECT tviews.pg_tviews_create('tv_twin', $$ SELECT pk, label FROM tb_twin $$) IS NOT NULL AS created;
SELECT must((SELECT t.id <> w.id FROM tv_tag t, tv_twin w WHERE t.pk_tag = 1 AND w.pk_twin = 1),
            'the entity is part of the derived id');

SELECT 'raw select stable id: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;

-- A TVIEW recomputed because a TVIEW it reads was replaced keeps its own text
-- search configuration, whatever the replacing session's
-- default_text_search_config.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_replace_dependents_text_search.sql
-- expect-output: replace_dependents_text_search: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'FAIL: %', msg; END IF; END $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text);
INSERT INTO tb_item (pk_item, title) VALUES (1, 'running dogs');

SET default_text_search_config = 'pg_catalog.english';
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('title', title) AS data FROM tb_item $$) \g /dev/null
SELECT tviews.pg_tviews_create('tv_note', $$
    SELECT i.pk_item AS pk_note, i.id,
           jsonb_build_object('words', to_tsvector(i.data->>'title')::text) AS data
    FROM tv_item i $$) \g /dev/null
SELECT data->>'words' AS english FROM tv_note WHERE pk_note = 1 \gset

SET default_text_search_config = 'pg_catalog.simple';
SELECT tviews.pg_tviews_create_or_replace('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('title', title) AS data FROM tb_item WHERE pk_item > 0 $$)
    \g /dev/null
SELECT must((SELECT data->>'words' FROM tv_note WHERE pk_note = 1) = :'english',
            'tv_note was recomputed under the replacing session''s configuration: '
            || (SELECT data->>'words' FROM tv_note WHERE pk_note = 1));

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_item;
\echo 'replace_dependents_text_search: PASS'

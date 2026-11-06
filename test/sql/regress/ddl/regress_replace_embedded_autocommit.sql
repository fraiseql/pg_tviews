-- Replacing a TVIEW that another TVIEW embeds, in its own transaction: the rows
-- the replace changes reach the embedding TVIEW before the statement ends, so
-- an implicit commit has nothing left to apply.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_replace_embedded_autocommit.sql
-- expect-output: replace embedded autocommit: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP TABLE IF EXISTS tb_item, tb_note CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text);
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_item int, body text);
INSERT INTO tb_item (pk_item, title) VALUES (1, 'a');
INSERT INTO tb_note (pk_note, fk_item, body) VALUES (1, 1, 'n');
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('title', title) AS data FROM tb_item $$) \g /dev/null
SELECT tviews.pg_tviews_create('tv_note', $$
    SELECT n.pk_note, n.id, jsonb_build_object('body', n.body, 'item', i.data) AS data
    FROM tb_note n LEFT JOIN tv_item i ON i.pk_item = n.fk_item $$) \g /dev/null

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'FAIL: %', msg; END IF; END $$;

SELECT must(tviews.pg_tviews_create_or_replace('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('title', upper(title)) AS data FROM tb_item $$)
            = 'replaced', 'the replace was not in place');
SELECT must((SELECT data->'item'->>'title' FROM tv_note WHERE pk_note = 1) = 'A',
            'tv_note is stale after an in-place replace');

-- A write after the replace still reaches both TVIEWs.
UPDATE tb_item SET title = 'b' WHERE pk_item = 1;
SELECT must((SELECT data->'item'->>'title' FROM tv_note WHERE pk_note = 1) = 'B',
            'tv_note is stale after a write following the replace');

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_item, tb_note CASCADE;
DROP FUNCTION must(boolean, text);
\echo 'replace embedded autocommit: PASS'

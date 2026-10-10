-- Replacing a TVIEW that embeds another one with a different column set.
--
-- The rebuild keeps the indexes a user created on the TVIEW's table and
-- recreates the ones pg_tviews manages, among them the index on each column
-- holding an embedded TVIEW's key, whatever that column is called.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_replace_embedding_tview.sql
-- expect-output: replace embedding TVIEW: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'notes');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user
$$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user AS author_pk,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user
$$);
CREATE INDEX my_post_title ON tv_post ((data->>'title'));

-- A new column: the table is rebuilt.
SELECT pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user AS author_pk, p.title,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user
$$);

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_indexes WHERE tablename = 'tv_post'
                                              AND indexname = 'my_post_title') THEN
        RAISE EXCEPTION 'FAIL: the index a user created is gone';
    END IF;
    IF (SELECT count(*) FROM pg_indexes WHERE tablename = 'tv_post'
                                          AND indexdef LIKE '%(author_pk%') <> 1 THEN
        RAISE EXCEPTION 'FAIL: not exactly one index on the embed key column: %',
            (SELECT string_agg(indexdef, '; ') FROM pg_indexes WHERE tablename = 'tv_post');
    END IF;
END $$;

UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
DO $$
BEGIN
    IF (SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 1) <> 'grace' THEN
        RAISE EXCEPTION 'FAIL: tv_post stale after the replace';
    END IF;
END $$;

\echo 'replace embedding TVIEW: PASS'

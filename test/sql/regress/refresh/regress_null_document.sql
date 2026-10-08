-- A backing view whose `data` is NULL for some rows: the TVIEW mirrors it.
--
-- `data` is nullable, like the view's column: a row whose document the view
-- computes as NULL is stored with a NULL document, on creation, on a write that
-- makes it NULL and on one that makes it a document again. A TVIEW embedding a
-- NULL document refreshes like any other, and a cascade into a parent whose own
-- document is NULL keeps it NULL. (An old refresh raised on a NULL document.)
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_null_document.sql
-- expect-output: null_document: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'null_document FAIL: %', what; END IF; END $$;

-- ── One table ───────────────────────────────────────────────────────────────
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'Widget'), (2, NULL);
SELECT pg_tviews_create('tv_item', $$
    SELECT pk_item, id,
           CASE WHEN name IS NOT NULL THEN jsonb_build_object('name', name) END AS data
    FROM tb_item $$);
SELECT must((SELECT data IS NULL FROM tv_item WHERE pk_item = 2), 'NULL document not stored on create');
SELECT assert_fresh('tv_item', 'pk_item', 'create');

UPDATE tb_item SET name = NULL WHERE pk_item = 1;
SELECT must((SELECT count(*) FROM tv_item WHERE data IS NULL) = 2, 'a document made NULL not stored');
SELECT assert_fresh('tv_item', 'pk_item', 'document -> NULL');

UPDATE tb_item SET name = 'back' WHERE pk_item = 2;
SELECT assert_fresh('tv_item', 'pk_item', 'NULL -> document');

-- ── A parent embedding a child whose document may be NULL ─────────────────
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, hidden boolean NOT NULL DEFAULT false);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada'), (2, 'bob');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, CASE WHEN NOT hidden THEN jsonb_build_object('name', name) END AS data
    FROM tb_user $$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           CASE WHEN p.title IS NOT NULL
                THEN jsonb_build_object('title', p.title, 'author', u.data) END AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);

-- The child's document becomes NULL, changes while NULL, comes back.
UPDATE tb_user SET hidden = true WHERE pk_user = 1;
SELECT must((SELECT data->'author' = 'null'::jsonb FROM tv_post WHERE pk_post = 1),
            'the embedded NULL document is not a JSON null');
SELECT assert_fresh('tv_user', 'pk_user', 'child document -> NULL');
SELECT assert_fresh('tv_post', 'pk_post', 'child document -> NULL');
UPDATE tb_user SET name = 'ada 2' WHERE pk_user = 1;
SELECT assert_fresh('tv_post', 'pk_post', 'a write to a child whose document is NULL');
UPDATE tb_user SET hidden = false WHERE pk_user = 1;
SELECT assert_fresh('tv_user', 'pk_user', 'child NULL -> document');
SELECT assert_fresh('tv_post', 'pk_post', 'child NULL -> document');

-- The parent's own document becomes NULL; a cascade into it keeps it NULL.
UPDATE tb_post SET title = NULL WHERE pk_post = 2;
SELECT assert_fresh('tv_post', 'pk_post', 'parent document -> NULL');
UPDATE tb_user SET name = 'bob 2' WHERE pk_user = 2;
SELECT must((SELECT data IS NULL FROM tv_post WHERE pk_post = 2),
            'a cascade into a NULL parent document made it non-NULL');
SELECT assert_fresh('tv_post', 'pk_post', 'a cascade into a NULL parent document');

-- A full refresh agrees.
SELECT pg_tviews_refresh('user');
SELECT assert_fresh('tv_user', 'pk_user', 'pg_tviews_refresh');
SELECT assert_fresh('tv_post', 'pk_post', 'pg_tviews_refresh');

\echo 'null_document: PASS'

-- A recomputed row's document replaces the stored one whole, also when the TVIEW
-- embeds another: a NULL document becomes the view's document, and a key the
-- view no longer produces goes. A merge of the fresh document into the stored
-- one (what a TVIEW with only scalar embeds got with jsonb_delta installed) kept
-- the NULL and the old key.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_document_replaced_whole.sql
-- expect-output: document_replaced_whole: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, NULL);
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           CASE WHEN p.title IS NOT NULL
                THEN jsonb_build_object('title', p.title, 'author', u.data) END AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);

UPDATE tb_post SET title = 'back' WHERE pk_post = 1;
SELECT assert_fresh('tv_post', 'pk_post', 'an own-column write to a NULL document');

-- A key the view stops producing.
CREATE TABLE tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     fk_user int NOT NULL REFERENCES tb_user, extra text);
INSERT INTO tb_doc (pk_doc, fk_user, extra) VALUES (1, 1, 'x');
SELECT pg_tviews_create('tv_doc', $$
    SELECT d.pk_doc, d.id, d.fk_user,
           jsonb_build_object('who', u.data->>'name')
           || CASE WHEN d.extra IS NOT NULL THEN jsonb_build_object('extra', d.extra)
                   ELSE '{}'::jsonb END AS data
    FROM tb_doc d JOIN tv_user u ON u.pk_user = d.fk_user $$);
UPDATE tb_doc SET extra = NULL WHERE pk_doc = 1;
SELECT assert_fresh('tv_doc', 'pk_doc', 'a write that removes a key from the document');

\echo 'document_replaced_whole: PASS'

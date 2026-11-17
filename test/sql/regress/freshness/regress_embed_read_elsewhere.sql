-- A TVIEW that embeds another's document and also reads a field of it elsewhere
-- (here, upper-cased under another key) gets both refreshed when the embedded
-- document changes, not only the embedded copy patched.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_embed_read_elsewhere.sql
--
-- known-failing: #321 (S30)
-- expect-output: embed read elsewhere: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann'), (2, 'ben');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) AS data FROM tb_user u $$) IS NOT NULL AS created;
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.data, 'shout', upper(u.data->>'name')) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$) IS NOT NULL AS created;

UPDATE tb_user SET name = 'anna' WHERE pk_user = 1;
SELECT assert_fresh('tv_user', 'pk_user', 'an UPDATE of tb_user');
SELECT assert_fresh('tv_post', 'pk_post', 'an UPDATE of the embedded document, also read under another key');

SELECT 'embed read elsewhere: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;

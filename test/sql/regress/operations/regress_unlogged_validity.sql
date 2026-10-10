-- An UNLOGGED TVIEW is refilled from its view only when PostgreSQL reset it (a
-- crash restart or a promotion empties every UNLOGGED table), never because it
-- happens to be empty (#214).
--
-- Each UNLOGGED TVIEW has a row in tviews.pg_tview_valid, itself UNLOGGED, so
-- the reset empties it together with the TVIEW's table. A missing row means the
-- table's contents can't be trusted: the first write claims the row and fills
-- the TVIEW, the TVIEWs it reads first. Deleting the row stands in for the reset
-- here; test/replication/promote_rebuild.sh runs a real one.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_unlogged_validity.sql
-- expect-output: unlogged_validity: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'unlogged_validity FAIL: %', what; END IF; END $$;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user VALUES (1, DEFAULT, 'ann'), (2, DEFAULT, 'bob');
INSERT INTO tb_post VALUES (1, DEFAULT, 1, 'p1'), (2, DEFAULT, 2, 'p2');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);

-- 1. Empty is not reset: a TVIEW whose rows were deleted is not refilled by the
--    first write of a new backend. (TRUNCATE leaves the TVIEW trusted.)
TRUNCATE tv_post;
\c
SET client_min_messages TO WARNING;
UPDATE tb_post SET title = 'p1!' WHERE pk_post = 1;
SELECT must((SELECT array_agg(pk_post ORDER BY pk_post) FROM tv_post) = '{1}',
            'an emptied but trusted tv_post was refilled from its view: '
            || (SELECT array_agg(pk_post ORDER BY pk_post) FROM tv_post)::text);
SELECT must(NOT needs_rebuild, 'an emptied but trusted TVIEW reported as needing a rebuild')
FROM pg_tviews_replication_status() WHERE entity = 'post';

-- 2. A reset (both TVIEWs, as a crash empties every UNLOGGED table): the first
--    write fills the TVIEW it refreshes and, first, the TVIEW it embeds.
TRUNCATE tv_user, tv_post;
DELETE FROM tviews.pg_tview_valid;
SELECT must((SELECT array_agg(entity ORDER BY entity) FROM pg_tviews_replication_status()
             WHERE needs_rebuild) = '{post,user}', 'reset TVIEWs not reported as needing a rebuild');
\c
SET client_min_messages TO WARNING;
UPDATE tb_post SET title = 'p2!' WHERE pk_post = 2;
SELECT must((SELECT count(*) FROM tv_post) = 2, 'the first write after a reset did not fill tv_post');
SELECT must((SELECT count(*) FROM tv_user) = 2, 'tv_post was filled before tv_user, which it reads');
SELECT assert_fresh('tv_user', 'pk_user', 'tv_user after a reset');
SELECT assert_fresh('tv_post', 'pk_post', 'tv_post after a reset');
SELECT must(NOT bool_or(needs_rebuild), 'a filled TVIEW still needs a rebuild')
FROM pg_tviews_replication_status();

-- 3. pg_tviews_recover_after_crash fills a reset TVIEW once, and only a reset one.
TRUNCATE tv_post;
SELECT must(NOT pg_tviews_recover_after_crash('post'), 'recovered a trusted TVIEW');
DELETE FROM tviews.pg_tview_valid
 WHERE table_oid = 'tv_post'::regclass;
SELECT must(pg_tviews_recover_after_crash('post'), 'a reset TVIEW was not recovered');
SELECT must(NOT pg_tviews_recover_after_crash('post'), 'recovered twice');
SELECT assert_fresh('tv_post', 'pk_post', 'tv_post after recover_after_crash');

-- 4. pg_tviews_rebuild_all() fills the reset TVIEWs only, dependencies first.
TRUNCATE tv_user, tv_post;
DELETE FROM tviews.pg_tview_valid WHERE table_oid = 'tv_user'::regclass;
SELECT must((SELECT array_agg(entity) FROM pg_tviews_rebuild_all()) = '{user}',
            'rebuild_all() did not fill exactly the reset tv_user');
SELECT must((SELECT count(*) FROM tv_post) = 0, 'rebuild_all() refilled the trusted tv_post');
SELECT assert_fresh('tv_user', 'pk_user', 'tv_user after rebuild_all');
SELECT pg_tviews_refresh('post');

-- 5. A LOGGED TVIEW has no row and is never refilled; switching persistence
--    keeps the rows right, and switching a reset TVIEW to LOGGED fills it first.
SELECT pg_tviews_set_logged('user', true);
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tview_valid WHERE table_oid = 'tv_user'::regclass),
            'a LOGGED TVIEW kept its row');
SELECT pg_tviews_set_logged('user', false);
SELECT must(EXISTS (SELECT 1 FROM tviews.pg_tview_valid WHERE table_oid = 'tv_user'::regclass),
            'a TVIEW switched to UNLOGGED has no row');
TRUNCATE tv_post;
DELETE FROM tviews.pg_tview_valid WHERE table_oid = 'tv_post'::regclass;
SELECT pg_tviews_set_logged('post', true);
SELECT assert_fresh('tv_post', 'pk_post', 'a reset TVIEW switched to LOGGED');
SELECT pg_tviews_set_logged('post', false);

-- 6. The row follows the TVIEW's table: created with it, gone with it.
SELECT must((SELECT count(*) FROM tviews.pg_tview_valid) = 2, 'not one row per UNLOGGED TVIEW');
SELECT pg_tviews_drop('tv_post');
SELECT must((SELECT count(*) FROM tviews.pg_tview_valid) = 1, 'a dropped TVIEW kept its row');
BEGIN;
SET LOCAL pg_tviews.unlogged_by_default = off;
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
COMMIT;
SELECT must((SELECT count(*) FROM tviews.pg_tview_valid) = 1, 'a LOGGED TVIEW got a row');

-- 7. Only pg_tviews writes the rows.
SELECT must(NOT has_table_privilege('public', 'tviews.pg_tview_valid', 'INSERT, DELETE, UPDATE'),
            'PUBLIC may write pg_tview_valid');

\echo 'unlogged_validity: PASS'

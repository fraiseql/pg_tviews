-- known-failing: #181
-- Regression test for issue #181: a backing view in tviews follows its TVIEW
-- through its life. Its name is derived from the TVIEW's table, so renaming the
-- table or moving it to another schema renames the view; dropping the TVIEW, its
-- schema or a view its definition reads leaves nothing behind in tviews; a
-- rebuild by pg_tviews_create_or_replace() keeps the view in tviews.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_181_lifecycle.sql
--
-- expect-output: issue #181 lifecycle: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE FUNCTION view_of(e text) RETURNS text LANGUAGE sql AS $$
    SELECT v.relnamespace::regnamespace::text || '.' || v.relname
    FROM tviews.pg_tview_meta m JOIN pg_class v ON v.oid = m.view_oid::oid WHERE m.entity = e $$;
CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#181 FAIL: %', what; END IF; END $$;

CREATE SCHEMA app;
CREATE SCHEMA app2;
CREATE TABLE app.tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO app.tb_post (pk_post, title) VALUES (1, 'a'), (2, 'b');
SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM app.tb_post $$);
SELECT must(view_of('post') = 'tviews.app__tv_post', 'created as ' || view_of('post'));

-- A rebuild keeps the view in tviews, under the same name.
SELECT must(tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT pk_post, id, title, jsonb_build_object('title', title) AS data FROM app.tb_post $$) = 'rebuilt',
            'a rebuild');
SELECT must(view_of('post') = 'tviews.app__tv_post', 'rebuilt as ' || view_of('post'));

-- SET SCHEMA moves the TVIEW: the view's name follows.
ALTER TABLE app.tv_post SET SCHEMA app2;
SELECT must(view_of('post') = 'tviews.app2__tv_post', 'after SET SCHEMA: ' || view_of('post'));
UPDATE app.tb_post SET title = 'a2' WHERE pk_post = 1;
SELECT assert_fresh('app2.tv_post', 'pk_post', 'a write after SET SCHEMA');

-- RENAME: the view's name follows the table's.
ALTER TABLE app2.tv_post RENAME TO tv_post_archive;
SELECT must(view_of('post') = 'tviews.app2__tv_post_archive', 'after RENAME: ' || view_of('post'));
UPDATE app.tb_post SET title = 'b2' WHERE pk_post = 2;
SELECT assert_fresh('app2.tv_post_archive', 'pk_post', 'a write after RENAME');
ALTER TABLE app2.tv_post_archive RENAME TO tv_post;
ALTER TABLE app2.tv_post SET SCHEMA app;
SELECT must(view_of('post') = 'tviews.app__tv_post', 'moved back: ' || view_of('post'));

-- pg_tviews_drop, then the same TVIEW again: the name is free.
SELECT tviews.pg_tviews_drop('app.tv_post');
SELECT must(to_regclass('tviews.app__tv_post') IS NULL, 'pg_tviews_drop left the view');
SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM app.tb_post $$);

-- DROP TABLE tv_* CASCADE.
DROP TABLE app.tv_post CASCADE;
SELECT must(to_regclass('tviews.app__tv_post') IS NULL AND NOT EXISTS (SELECT 1 FROM tviews.registry),
            'DROP TABLE tv_post CASCADE left the view or the registration');

-- A view the definition reads, dropped with CASCADE: the TVIEW goes with it.
CREATE VIEW app.v_titles AS SELECT pk_post, title FROM app.tb_post;
SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT t.pk_post, p.id, jsonb_build_object('title', t.title) AS data
    FROM app.v_titles t JOIN app.tb_post p USING (pk_post) $$);
DROP VIEW app.v_titles CASCADE;
SELECT must(to_regclass('tviews.app__tv_post') IS NULL AND NOT EXISTS (SELECT 1 FROM tviews.registry),
            'DROP VIEW v_titles CASCADE left the backing view or the registration');

-- The application schema dropped: nothing of it is left in tviews.
SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM app.tb_post $$);
DROP SCHEMA app CASCADE;
SELECT must(NOT EXISTS (SELECT 1 FROM pg_class WHERE relnamespace = 'tviews'::regnamespace
                        AND relname LIKE 'app\_\_%'),
            'DROP SCHEMA app CASCADE left a view in tviews');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tview_meta), 'DROP SCHEMA app CASCADE left a registration');

\echo 'issue #181 lifecycle: PASS'

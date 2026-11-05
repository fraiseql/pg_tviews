-- A TVIEW is re-registered on the table it is registered with, found by OID:
-- renaming that table keeps re-registration working, and a relation that later
-- takes the table's first name never becomes the TVIEW's table.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_reregister_by_oid.sql
-- expect-output: reregister_by_oid: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS rbo CASCADE;
DROP ROLE IF EXISTS regress_rbo_other;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'FAIL: %', msg; END IF; END $$;

CREATE ROLE regress_rbo_other;
CREATE SCHEMA rbo;
GRANT USAGE, CREATE ON SCHEMA rbo TO regress_rbo_other;
CREATE TABLE rbo.tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          title text);
INSERT INTO rbo.tb_post (pk_post, title) VALUES (1, 'one');
SELECT tviews.pg_tviews_create('rbo.tv_post', $$
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM rbo.tb_post $$) \g /dev/null
SELECT table_oid::oid AS registered FROM tviews.pg_tview_meta WHERE entity = 'post' \gset

ALTER TABLE rbo.tv_post RENAME TO tv_post_live;
SELECT tviews.pg_tviews_reregister('post') \g /dev/null
SELECT must((SELECT table_oid::oid FROM tviews.pg_tview_meta WHERE entity = 'post') = :'registered'::oid,
            'a renamed TVIEW table was not re-registered on itself');

-- Another role's table takes the first name.
SET ROLE regress_rbo_other;
CREATE TABLE rbo.tv_post (pk_post int PRIMARY KEY, id uuid, data jsonb);
RESET ROLE;
SELECT tviews.pg_tviews_reregister('post') \g /dev/null
SELECT must((SELECT table_oid::oid FROM tviews.pg_tview_meta WHERE entity = 'post') = :'registered'::oid,
            're-registration bound the TVIEW to the table that took its first name');
UPDATE rbo.tb_post SET title = 'two';
SELECT must((SELECT data->>'title' FROM rbo.tv_post_live) = 'two', 'tv_post_live is not refreshed');
SELECT must(NOT EXISTS (SELECT 1 FROM rbo.tv_post), 'the other role''s table was written');

-- A TVIEW owner without CREATE on the extension's schema renames its table:
-- the backing view follows.
GRANT USAGE ON SCHEMA rbo TO regress_rbo_other;
SET ROLE regress_rbo_other;
CREATE TABLE rbo.tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         label text);
INSERT INTO rbo.tb_tag (pk_tag, label) VALUES (1, 't');
SELECT tviews.pg_tviews_create('rbo.tv_tag', $$
    SELECT pk_tag, id, jsonb_build_object('label', label) AS data FROM rbo.tb_tag $$) \g /dev/null
ALTER TABLE rbo.tv_tag RENAME TO tv_tag_live;
RESET ROLE;
SELECT must((SELECT view FROM tviews.registry WHERE name = 'tv_tag_live') IS NOT NULL,
            'the backing view did not follow the rename');

DROP TABLE rbo.tv_post;
SELECT tviews.pg_tviews_drop('tag');
DROP EXTENSION pg_tviews CASCADE;
DROP SCHEMA rbo CASCADE;
DROP ROLE regress_rbo_other;

\echo 'reregister_by_oid: PASS'

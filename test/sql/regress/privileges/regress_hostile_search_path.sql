-- Regression test (#271): no SQL pg_tviews runs resolves a name under the caller's
-- search_path.
--
-- A schema first on the caller's path, before pg_catalog, shadows what pg_tviews'
-- SQL is made of: `=` and `||` on text, oid and name, `jsonb_build_object`,
-- `format_type`, `quote_ident`. Every shadow raises. Creating, writing, refreshing,
-- reporting, resyncing and checking health under that path touch none of them,
-- and leave the same TVIEW rows as the views compute.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_hostile_search_path.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS hostile CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'hostile search_path FAIL: %', what; END IF; END $$;

CREATE SCHEMA hostile;
CREATE FUNCTION hostile.shadow(what text) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'hostile search_path FAIL: % resolved in schema hostile', what; END $$;
CREATE FUNCTION hostile.eq_text(text, text) RETURNS boolean LANGUAGE sql
    AS $$ SELECT hostile.shadow('text = text') $$;
CREATE FUNCTION hostile.eq_oid(oid, oid) RETURNS boolean LANGUAGE sql
    AS $$ SELECT hostile.shadow('oid = oid') $$;
CREATE FUNCTION hostile.eq_name(name, name) RETURNS boolean LANGUAGE sql
    AS $$ SELECT hostile.shadow('name = name') $$;
CREATE FUNCTION hostile.eq_name_text(name, text) RETURNS boolean LANGUAGE sql
    AS $$ SELECT hostile.shadow('name = text') $$;
CREATE FUNCTION hostile.cat_text(text, text) RETURNS text LANGUAGE sql
    AS $$ SELECT hostile.shadow('text || text')::text $$;
CREATE OPERATOR hostile.= (LEFTARG = text, RIGHTARG = text, FUNCTION = hostile.eq_text);
CREATE OPERATOR hostile.= (LEFTARG = oid, RIGHTARG = oid, FUNCTION = hostile.eq_oid);
CREATE OPERATOR hostile.= (LEFTARG = name, RIGHTARG = name, FUNCTION = hostile.eq_name);
CREATE OPERATOR hostile.= (LEFTARG = name, RIGHTARG = text, FUNCTION = hostile.eq_name_text);
CREATE OPERATOR hostile.|| (LEFTARG = text, RIGHTARG = text, FUNCTION = hostile.cat_text);
CREATE FUNCTION hostile.jsonb_build_object(text, text) RETURNS jsonb LANGUAGE sql
    AS $$ SELECT to_jsonb(hostile.shadow('jsonb_build_object(text, text)')) $$;
CREATE FUNCTION hostile.jsonb_build_object(text, jsonb) RETURNS jsonb LANGUAGE sql
    AS $$ SELECT to_jsonb(hostile.shadow('jsonb_build_object(text, jsonb)')) $$;
CREATE FUNCTION hostile.format_type(oid, integer) RETURNS text LANGUAGE sql
    AS $$ SELECT hostile.shadow('format_type')::text $$;
CREATE FUNCTION hostile.quote_ident(text) RETURNS text LANGUAGE sql
    AS $$ SELECT hostile.shadow('quote_ident')::text $$;
GRANT USAGE ON SCHEMA hostile TO PUBLIC;

-- The shadows bite: an unqualified `=` on text under that path raises.
SET search_path = hostile, pg_catalog, public;
DO $$ BEGIN
    PERFORM 'a'::text = 'a'::text;
    RAISE EXCEPTION 'hostile search_path FAIL: the shadow did not resolve';
EXCEPTION WHEN raise_exception THEN
    IF SQLERRM NOT LIKE '%text = text resolved in schema hostile%' THEN RAISE; END IF;
END $$;
RESET search_path;

-- User ← Post, the post embedding its author.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                      name text NOT NULL);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text NOT NULL);
CREATE INDEX ON tb_post (fk_user);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 1, 'p2'), (3, 2, 'p3');
CREATE VIEW v_user AS
    SELECT u.pk_user, u.id, jsonb_build_object('id', u.id, 'name', u.name, 'n', 1) AS data
    FROM tb_user u;
CREATE VIEW v_post AS
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('id', p.id, 'title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN v_user u ON u.pk_user = p.fk_user;

-- Whether every TVIEW holds what its view computes, read under a clean path.
SET check_function_bodies = off;
CREATE FUNCTION in_sync() RETURNS boolean LANGUAGE sql SET search_path = pg_catalog, public AS $$
    SELECT NOT EXISTS (SELECT pk_user, data FROM v_user EXCEPT SELECT pk_user, data FROM tv_user)
       AND NOT EXISTS (SELECT pk_user, data FROM tv_user EXCEPT SELECT pk_user, data FROM v_user)
       AND NOT EXISTS (SELECT pk_post, data FROM v_post EXCEPT SELECT pk_post, data FROM tv_post)
       AND NOT EXISTS (SELECT pk_post, data FROM tv_post EXCEPT SELECT pk_post, data FROM v_post) $$;
RESET check_function_bodies;

SET search_path = hostile, pg_catalog, public;

-- 1. Create.
SELECT tviews.pg_tviews_create('public.tv_user', 'SELECT pk_user, id, data FROM public.v_user');
SELECT tviews.pg_tviews_create('public.tv_post',
                               'SELECT pk_post, id, fk_user, data FROM public.v_post');
SELECT must(in_sync(), 'after create');

-- 2. Writes: insert, update (cascading into posts), delete, in one transaction and alone.
BEGIN;
INSERT INTO tb_user (pk_user, name) VALUES (3, 'carol');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (4, 3, 'p4');
UPDATE tb_user SET name = 'alicia' WHERE pk_user = 1;
COMMIT;
SELECT must(in_sync(), 'after the write transaction');
UPDATE tb_post SET fk_user = 2 WHERE pk_post = 2;
DELETE FROM tb_post WHERE pk_post = 4;
SELECT must(in_sync(), 'after single writes');

-- 3. Refresh.
SELECT tviews.pg_tviews_refresh('user');
SELECT must(in_sync(), 'after refresh');

-- 4. Report, with data and ids only, and with types.
BEGIN;
UPDATE tb_user SET name = 'bobby' WHERE pk_user = 2;
SELECT tviews.pg_tviews_flush_and_report(reset => false) AS r \gset
SELECT tviews.pg_tviews_flush_and_report(include_data => false, reset => false) AS r2 \gset
SELECT tviews.pg_tviews_flush_and_report(types => ARRAY['Post']) AS r3 \gset
COMMIT;
RESET search_path;
SELECT must(jsonb_array_length((:'r'::jsonb)->'updated') = 3, 'report: ' || :'r');
SELECT must(jsonb_array_length((:'r2'::jsonb)->'updated') = 3, 'report ids: ' || :'r2');
SELECT must(jsonb_array_length((:'r3'::jsonb)->'updated') = 2, 'report types: ' || :'r3');
SELECT must(in_sync(), 'after the report');
SET search_path = hostile, pg_catalog, public;

-- 5. Resync, after a write no trigger saw.
RESET search_path;
SET session_replication_role = replica;
UPDATE tb_user SET name = 'resynced' WHERE pk_user = 1;
RESET session_replication_role;
SET search_path = hostile, pg_catalog, public;
SELECT count(*) AS resynced FROM tviews.pg_tviews_resync('public.tb_user', ARRAY[1]) \gset
SELECT must(in_sync(), 'after resync');

-- 6. Health, profile, read sets: the read-only entry points.
SELECT count(*) AS health FROM tviews.pg_tviews_check_health() \gset
SELECT count(*) AS profiled FROM tviews.pg_tviews_profile() \gset
SELECT count(*) AS shown FROM tviews.pg_tviews_show_queue() \gset
RESET search_path;
SELECT must(:health > 0, 'health returned nothing');
SELECT must(NOT EXISTS (SELECT FROM tviews.pg_tviews_check_health() WHERE status = 'ERROR'),
            'health reports an error');

-- 7. Drop.
SET search_path = hostile, pg_catalog, public;
SELECT tviews.pg_tviews_drop('public.tv_post');
SELECT tviews.pg_tviews_drop('public.tv_user');
RESET search_path;
SELECT must(to_regclass('public.tv_post') IS NULL, 'tv_post not dropped');

DROP EXTENSION pg_tviews CASCADE;
DROP SCHEMA hostile CASCADE;
\echo 'regress_hostile_search_path: PASS'

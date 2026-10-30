-- A writer's search_path has no say in the SQL a refresh runs as the TVIEW's
-- owner: what the write path reads from the catalog (a key's type, a relation's
-- name, a column's type) is read under search_path = pg_catalog, pg_temp.
--
-- Read under the writer's path, a join column of type app.myid rendered as `myid`
-- when that path named app, and the owner's statement, under its own path, failed
-- with `type "myid" does not exist`. A type or an operator of the writer's (a
-- domain in pg_temp, `||` in a schema before pg_catalog) is never used.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_writer_search_path.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS app CASCADE;
DROP SCHEMA IF EXISTS regress_wsp_writer CASCADE;
DROP TABLE IF EXISTS public.ran_as;
DROP ROLE IF EXISTS regress_wsp_writer;
DROP ROLE IF EXISTS regress_wsp_owner;
CREATE ROLE regress_wsp_owner;
CREATE ROLE regress_wsp_writer;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
GRANT USAGE ON SCHEMA tviews TO regress_wsp_owner;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'writer search_path FAIL: %', what; END IF; END $$;

-- Who ran the writer's code. Writable by anyone, so the writer's code can record
-- whichever role runs it.
CREATE TABLE public.ran_as (who text);
GRANT INSERT, SELECT ON public.ran_as TO PUBLIC;

CREATE SCHEMA app AUTHORIZATION regress_wsp_owner;
-- The writer's own schema: "$user" on its search_path.
CREATE SCHEMA regress_wsp_writer AUTHORIZATION regress_wsp_writer;
GRANT USAGE ON SCHEMA regress_wsp_writer TO PUBLIC;

SET ROLE regress_wsp_owner;
CREATE DOMAIN app.myid AS int;
CREATE TABLE app.tb_user (pk_user app.myid PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
CREATE TABLE app.tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          fk_user app.myid REFERENCES app.tb_user, title text);
CREATE TABLE app.tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         title text);
INSERT INTO app.tb_user VALUES (1, DEFAULT, 'alice');
INSERT INTO app.tb_post VALUES (1, DEFAULT, 1, 'hello');
INSERT INTO app.tb_doc VALUES (1, DEFAULT, 'a');
SELECT tviews.pg_tviews_create('app.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM app.tb_post p JOIN app.tb_user u ON u.pk_user = p.fk_user $$) IS NOT NULL;
-- Keyed on its uuid (ADR 0169): the key type is read from the catalog.
SELECT tviews.pg_tviews_create('app.tv_doc', $$
    SELECT DISTINCT ON (d.id) d.pk_doc, d.id, jsonb_build_object('title', d.title) AS data
    FROM app.tb_doc d ORDER BY d.id, d.pk_doc DESC $$) IS NOT NULL;
GRANT USAGE ON SCHEMA app TO regress_wsp_writer;
GRANT SELECT, INSERT, UPDATE ON app.tb_user, app.tb_post, app.tb_doc TO regress_wsp_writer;
RESET ROLE;

-- 1. An ordinary writer whose search_path names the schema of a join column's
--    type: its write succeeds and the TVIEW follows.
\c
SET ROLE regress_wsp_writer;
SET search_path = app, public;
INSERT INTO tb_user VALUES (2, DEFAULT, 'bob');
INSERT INTO tb_post VALUES (2, DEFAULT, 2, 'second');
UPDATE tb_user SET name = 'robert' WHERE pk_user = 2;
RESET ROLE;
RESET search_path;
SELECT must((SELECT data->>'author' FROM app.tv_post WHERE pk_post = 2) = 'robert',
            'tv_post does not follow a write made under search_path app, public');

-- 2. A domain of the writer's in pg_temp, named like the join column's type.
\c
SET ROLE regress_wsp_writer;
CREATE FUNCTION pg_temp.record() RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN INSERT INTO public.ran_as VALUES (current_user); RETURN true; END $$;
CREATE DOMAIN pg_temp.myid AS int CHECK (pg_temp.record());
SET search_path = app, public, pg_temp;
INSERT INTO tb_user VALUES (5, DEFAULT, 'erin');
INSERT INTO tb_post VALUES (5, DEFAULT, 5, 'fifth');
RESET ROLE;
RESET search_path;
SELECT must(NOT EXISTS (SELECT FROM public.ran_as WHERE who = 'regress_wsp_owner'),
            'a pg_temp domain of the writer ran as the TVIEW owner');
SELECT must((SELECT data->>'author' FROM app.tv_post WHERE pk_post = 5) = 'erin',
            'tv_post does not follow a write made with a pg_temp domain');

-- 3. A text || text of the writer's, searched before pg_catalog: recorded when
--    called, never by pg_tviews.
\c
SET ROLE regress_wsp_writer;
CREATE FUNCTION cat(a text, b text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO public.ran_as VALUES (current_user);
    RETURN pg_catalog.textcat(a, b);
END $$;
CREATE OPERATOR || (LEFTARG = text, RIGHTARG = text, FUNCTION = cat);
SET search_path = regress_wsp_writer, pg_catalog, app;
UPDATE tb_doc SET title = 'b' WHERE pk_doc = 1;
INSERT INTO tb_user VALUES (6, DEFAULT, 'frank');
INSERT INTO tb_post VALUES (6, DEFAULT, 6, 'sixth');
RESET ROLE;
RESET search_path;
SELECT must(NOT EXISTS (SELECT FROM public.ran_as),
            'pg_tviews called an operator of the writer');
SELECT must((SELECT data->>'title' FROM app.tv_doc) = 'b',
            'tv_doc does not follow a write made with an operator of the writer');
SELECT must((SELECT data->>'author' FROM app.tv_post WHERE pk_post = 6) = 'frank',
            'tv_post does not follow a write made with an operator of the writer');

DROP SCHEMA app CASCADE;
DROP SCHEMA regress_wsp_writer CASCADE;
DROP TABLE public.ran_as;
REVOKE USAGE ON SCHEMA tviews FROM regress_wsp_owner;
DROP ROLE regress_wsp_writer;
DROP ROLE regress_wsp_owner;

-- Regression test (#122): refresh must not depend on the session's search_path.
--
-- The refresh path used to name tv_<entity>, v_<entity> and the extension's own
-- pg_tview_meta unqualified, so DML on the base table of a TVIEW living in a schema
-- that is not on search_path failed with `relation "tv_post" does not exist`.
-- Every relation (and jsonb_delta patch call) is now schema-qualified.
--
-- Each block reconnects first, so no per-backend cache warmed while the TVIEWs'
-- schema was still on search_path can hide the bug.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_122_search_path.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP SCHEMA IF EXISTS app CASCADE;
CREATE SCHEMA app;
SET search_path TO app, public, tviews;

CREATE TABLE app.tb_user (
    pk_user int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL,
    bio     text
);
CREATE TABLE app.tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES app.tb_user,
    title   text NOT NULL
);
CREATE TABLE app.tb_tag (
    pk_tag  int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_post int NOT NULL REFERENCES app.tb_post,
    label   text NOT NULL
);
INSERT INTO app.tb_user (pk_user, name, bio) VALUES (1, 'alice', 'a'), (2, 'bob', 'b');
INSERT INTO app.tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 1, 'p2'), (3, 2, 'p3');
INSERT INTO app.tb_tag (pk_tag, fk_post, label) VALUES (1, 1, 't1'), (2, 1, 't2');

-- Scalar own columns: direct-patch fast path.
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data
    FROM app.tb_user $$);
-- Nested object from another TVIEW: cascade + full-row refresh.
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM app.tb_post p JOIN app.v_user u ON u.pk_user = p.fk_user $$);
-- Array aggregate: parent lookups through propagation.
SELECT pg_tviews_create('tv_tag', $$
    SELECT t.pk_tag, t.id, t.fk_post,
           jsonb_build_object('label', t.label, 'post', p.data->'title') AS data
    FROM app.tb_tag t JOIN app.v_post p ON p.pk_post = t.fk_post $$);

-- Rows of tv and v that differ in either direction.
CREATE FUNCTION public.assert_122(entity text, step text) RETURNS void
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE d bigint;
BEGIN
    EXECUTE format(
        'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM app.tv_%1$s
                                EXCEPT SELECT pk_%1$s, data FROM app.v_%1$s)
                     UNION ALL (SELECT pk_%1$s, data FROM app.v_%1$s
                                EXCEPT SELECT pk_%1$s, data FROM app.tv_%1$s)) d',
        entity) INTO d;
    IF d <> 0 THEN
        RAISE EXCEPTION '#122 FAIL after %: app.tv_% diverges from app.v_% (% rows)',
            step, entity, entity, d;
    END IF;
END $$;

CREATE FUNCTION public.assert_all_122(step text) RETURNS void
LANGUAGE sql SET search_path = pg_catalog AS $$
    SELECT public.assert_122('user', step);
    SELECT public.assert_122('post', step);
    SELECT public.assert_122('tag', step);
$$;

-- (1) search_path without the TVIEWs' schema (the default "$user", public).
\c
SET client_min_messages TO WARNING;
SHOW search_path;
UPDATE app.tb_user SET bio = 'a2' WHERE pk_user = 1;              -- direct patch
UPDATE app.tb_user SET name = 'alice2' WHERE pk_user = 1;         -- cascade to post, tag
UPDATE app.tb_post SET title = title || '!';                      -- multi-row + cascade
INSERT INTO app.tb_post (pk_post, fk_user, title) VALUES (4, 2, 'p4');
INSERT INTO app.tb_tag (pk_tag, fk_post, label) VALUES (3, 4, 't3');
DELETE FROM app.tb_tag WHERE pk_tag = 2;
SELECT public.assert_all_122('DML with search_path = "$user", public');

-- Explicit transaction: flushed at COMMIT by the ProcessUtility hook.
BEGIN;
UPDATE app.tb_user SET name = 'bob2' WHERE pk_user = 2;
UPDATE app.tb_post SET fk_user = 1 WHERE pk_post = 3;
COMMIT;
SELECT public.assert_all_122('explicit transaction, public only');

-- (2) search_path with neither the TVIEWs' schema nor the extensions' schema.
\c
SET client_min_messages TO WARNING;
SET search_path = pg_catalog;
UPDATE app.tb_user SET bio = 'b3' WHERE pk_user = 2;
UPDATE app.tb_user SET name = 'alice3' WHERE pk_user = 1;
UPDATE app.tb_post SET title = 'moved' WHERE pk_post = 4;
INSERT INTO app.tb_user (pk_user, name) VALUES (3, 'carol');
INSERT INTO app.tb_post (pk_post, fk_user, title) VALUES (5, 3, 'p5');
DELETE FROM app.tb_tag WHERE pk_tag = 3;
DELETE FROM app.tb_post WHERE pk_post = 4;
SELECT public.assert_all_122('DML with search_path = pg_catalog');

BEGIN;
UPDATE app.tb_user SET name = 'carol2' WHERE pk_user = 3;
DELETE FROM app.tb_post WHERE pk_post = 5;
COMMIT;
SELECT public.assert_all_122('explicit transaction, pg_catalog only');

-- Full refresh and catalog functions resolve their relations too.
SELECT tviews.pg_tviews_refresh('post');
SELECT public.assert_all_122('pg_tviews_refresh, pg_catalog only');

RESET search_path;
SELECT 'issue #122 search_path: PASS' AS result;
-- expect-output: issue #122 search_path: PASS

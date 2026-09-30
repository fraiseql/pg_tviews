-- Regression test (#128): the preloaded hook leaves databases without the
-- extension alone.
--
-- pg_tviews is loaded through shared_preload_libraries, so its ProcessUtility
-- hook runs in every database. Where the extension is not installed it used to
-- look tables up in pg_tview_meta anyway, and DROP TABLE failed with
-- `relation "pg_tview_meta" does not exist`.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_128_without_extension.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;

-- Plain DDL the hook inspects: DROP TABLE, RENAME COLUMN, CREATE TABLE tv_* AS.
CREATE TABLE tb_thing (pk_thing int PRIMARY KEY, id uuid DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_thing (pk_thing, name) VALUES (1, 'a');
ALTER TABLE tb_thing RENAME COLUMN name TO label;
CREATE TABLE tv_thing AS
    SELECT pk_thing, id, jsonb_build_object('label', label) AS data FROM tb_thing;
DROP TABLE tv_thing;
DROP TABLE tb_thing;

DO $$ BEGIN
    IF to_regclass('tv_thing') IS NOT NULL OR to_regclass('v_thing') IS NOT NULL THEN
        RAISE EXCEPTION '#128 FAIL: CREATE TABLE tv_* AS was converted without the extension';
    END IF;
END $$;

-- Installing the extension afterwards still works, and so does its hook.
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE TABLE tb_thing (pk_thing int PRIMARY KEY, id uuid DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_thing (pk_thing, name) VALUES (1, 'a');
SELECT pg_tviews_create('tv_thing', $$
    SELECT pk_thing, id, jsonb_build_object('name', name) AS data FROM tb_thing $$);
UPDATE tb_thing SET name = 'b';
DO $$ BEGIN
    IF (SELECT data->>'name' FROM tv_thing) <> 'b' THEN
        RAISE EXCEPTION '#128 FAIL: tv_thing not refreshed after reinstalling';
    END IF;
END $$;
DROP TABLE tv_thing;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'thing') THEN
        RAISE EXCEPTION '#128 FAIL: DROP TABLE tv_thing left its metadata';
    END IF;
END $$;

-- Dropped again: DDL keeps working.
DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_thing;

SELECT 'issue #128 without extension: PASS' AS result;
-- expect-output: issue #128 without extension: PASS

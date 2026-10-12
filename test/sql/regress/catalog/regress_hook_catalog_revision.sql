-- Regression test (#227): DDL the hook intercepts checks the catalog revision.
--
-- A database whose installed catalog is of another revision than the preloaded
-- library (an older install under a newer package) must fail DROP / ALTER TABLE
-- on a TVIEW with the revision error and its remedy, before the hook reads the
-- catalog. It used to read the catalog as if it were its own.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_hook_catalog_revision.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_thing (pk_thing int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_thing (pk_thing, name) VALUES (1, 'a');
SELECT tviews.pg_tviews_create('tv_thing', $$
    SELECT pk_thing, id, jsonb_build_object('name', name) AS data FROM tb_thing $$);

CREATE TABLE saved_revision AS SELECT tviews.pg_tviews_catalog_revision() AS r;
CREATE FUNCTION set_revision(r int) RETURNS void LANGUAGE plpgsql AS $f$
BEGIN
    EXECUTE format('CREATE OR REPLACE FUNCTION tviews.pg_tviews_catalog_revision()
                    RETURNS integer LANGUAGE sql IMMUTABLE AS %L', 'SELECT ' || r);
END $f$;
-- The error and hint `stmt` raises, or NULL.
CREATE FUNCTION ddl_error(stmt text) RETURNS text LANGUAGE plpgsql AS $f$
DECLARE msg text; hint text;
BEGIN
    EXECUTE stmt;
    RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, hint = PG_EXCEPTION_HINT;
    RETURN msg || ' / ' || hint;
END $f$;

CREATE TABLE plain_t (a int);
SELECT set_revision((SELECT r - 1 FROM saved_revision));
\c
SET client_min_messages TO WARNING;
DO $$
DECLARE stmt text; got text;
BEGIN
    FOREACH stmt IN ARRAY ARRAY[
        'DROP TABLE tv_thing',
        'DROP TABLE IF EXISTS tv_thing CASCADE',
        'ALTER TABLE tv_thing RENAME COLUMN data TO doc',
        'ALTER TABLE tv_thing SET UNLOGGED',
        -- Whether a table is a TVIEW is the catalog's to say: any DROP TABLE waits
        -- for the catalog update rather than risk leaving a TVIEW's row behind.
        'DROP TABLE plain_t'
    ] LOOP
        got := ddl_error(stmt);
        IF coalesce(got, '') NOT LIKE
           '%library catalog revision%does not match the installed extension%ALTER EXTENSION pg_tviews UPDATE%' THEN
            RAISE EXCEPTION '#227 FAIL: % under an older catalog: %', stmt, coalesce(got, 'no error');
        END IF;
    END LOOP;
END $$;

-- DDL the hook does not look up still runs.
CREATE TABLE plain_u (a int);
ALTER TABLE plain_u ADD COLUMN b int;

-- The matching revision back: the TVIEW drops as usual.
DO $$ BEGIN
    EXECUTE format('CREATE OR REPLACE FUNCTION tviews.pg_tviews_catalog_revision()
                    RETURNS integer LANGUAGE sql IMMUTABLE AS %L',
                   'SELECT ' || (SELECT r FROM saved_revision));
END $$;
DROP TABLE tv_thing, plain_t, plain_u;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'thing') THEN
        RAISE EXCEPTION '#227 FAIL: DROP TABLE tv_thing left its catalog row';
    END IF;
END $$;

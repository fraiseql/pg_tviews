-- Regression test (#136): the extension lives in the fixed schema `tviews`.
--
-- CREATE EXTENSION used to install into the first schema on search_path, with a
-- few objects hardcoded to `public`. Every object now lives in `tviews`, the
-- install script refuses to reuse objects it did not create, and nothing
-- pg_tviews does needs `tviews` (or `public`) on search_path. Base-table trigger
-- names fit PostgreSQL's 63 bytes without colliding, and a dropped TVIEW's
-- triggers are found however long or multibyte its names are.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_136_schema.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS tviews CASCADE;
DROP SCHEMA IF EXISTS app CASCADE;
DROP SCHEMA IF EXISTS "schéma_très_long_pour_les_noms_de_déclencheurs" CASCADE;
DROP ROLE IF EXISTS regress_136_other;
CREATE SCHEMA app;
CREATE EXTENSION jsonb_delta;

-- 1. The install schema does not follow search_path.
SET search_path TO app, public;
CREATE EXTENSION pg_tviews;

DO $$
DECLARE
    stray text;
BEGIN
    IF (SELECT n.nspname FROM pg_catalog.pg_extension e
        JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace
        WHERE e.extname = 'pg_tviews') IS DISTINCT FROM 'tviews' THEN
        RAISE EXCEPTION '#136 FAIL: pg_tviews not installed in schema tviews';
    END IF;
    SELECT string_agg(pg_catalog.pg_describe_object(d.classid, d.objid, 0), ', ')
      INTO stray
      FROM pg_catalog.pg_depend d
      JOIN pg_catalog.pg_extension e ON e.oid = d.refobjid
     WHERE d.refclassid = 'pg_catalog.pg_extension'::pg_catalog.regclass
       AND e.extname = 'pg_tviews' AND d.deptype = 'e'
       AND COALESCE(
             (SELECT relnamespace FROM pg_catalog.pg_class WHERE oid = d.objid
               AND d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass),
             (SELECT pronamespace FROM pg_catalog.pg_proc WHERE oid = d.objid
               AND d.classid = 'pg_catalog.pg_proc'::pg_catalog.regclass),
             (SELECT typnamespace FROM pg_catalog.pg_type WHERE oid = d.objid
               AND d.classid = 'pg_catalog.pg_type'::pg_catalog.regclass),
             'tviews'::pg_catalog.regnamespace) <> 'tviews'::pg_catalog.regnamespace;
    IF stray IS NOT NULL THEN
        RAISE EXCEPTION '#136 FAIL: extension members outside tviews: %', stray;
    END IF;
END $$;

-- 2. WITH SCHEMA is refused; DROP EXTENSION leaves tviews behind, empty.
DROP EXTENSION pg_tviews;
DO $$ BEGIN
    IF to_regnamespace('tviews') IS NULL
       OR EXISTS (SELECT 1 FROM pg_catalog.pg_class
                  WHERE relnamespace = 'tviews'::pg_catalog.regnamespace)
       OR EXISTS (SELECT 1 FROM pg_catalog.pg_proc
                  WHERE pronamespace = 'tviews'::pg_catalog.regnamespace) THEN
        RAISE EXCEPTION '#136 FAIL: DROP EXTENSION did not leave an empty tviews schema';
    END IF;
    BEGIN
        CREATE EXTENSION pg_tviews SCHEMA app;
    EXCEPTION WHEN OTHERS THEN
        IF SQLERRM LIKE '%must be installed in schema "tviews"%' THEN
            RETURN;
        END IF;
        RAISE;
    END;
    RAISE EXCEPTION '#136 FAIL: CREATE EXTENSION pg_tviews SCHEMA app was accepted';
END $$;

-- 3. The install script neither adopts a schema owned by another role nor
--    reuses an object it did not create.
DROP SCHEMA tviews;
CREATE ROLE regress_136_other;
CREATE SCHEMA tviews AUTHORIZATION regress_136_other;
DO $$ BEGIN
    BEGIN
        CREATE EXTENSION pg_tviews;
    EXCEPTION WHEN OTHERS THEN
        IF SQLERRM LIKE '%owned by%regress_136_other%' THEN
            RETURN;
        END IF;
        RAISE;
    END;
    RAISE EXCEPTION '#136 FAIL: installed into a tviews schema owned by another role';
END $$;
DROP SCHEMA tviews;
DROP ROLE regress_136_other;

CREATE SCHEMA tviews;
CREATE VIEW tviews.pg_tviews_cache_stats AS SELECT 1 AS planted;
DO $$ BEGIN
    BEGIN
        CREATE EXTENSION pg_tviews;
    EXCEPTION WHEN duplicate_table THEN
        RETURN;
    END;
    RAISE EXCEPTION '#136 FAIL: install reused a pre-existing tviews.pg_tviews_cache_stats';
END $$;
DROP SCHEMA tviews CASCADE;
CREATE EXTENSION pg_tviews;

-- 4. Nothing needs tviews or public on search_path.
CREATE TABLE app.tb_user (
    pk_user int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL
);
CREATE TABLE app.tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES app.tb_user,
    title   text NOT NULL
);
INSERT INTO app.tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO app.tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');

CREATE FUNCTION app.assert_136(entity text, step text) RETURNS void
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
        RAISE EXCEPTION '#136 FAIL after %: app.tv_% diverges from app.v_% (% rows)',
            step, entity, entity, d;
    END IF;
END $$;

-- Created with the function, unqualified name resolving to current_schema().
SET search_path TO app;
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM app.tb_user $$);

-- Created through CTAS, with an empty search_path.
SET search_path TO '';
CREATE TABLE app.tv_post AS
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM app.tb_post p JOIN app.v_user u ON u.pk_user = p.fk_user;
SELECT app.assert_136('post', 'CTAS');

-- A fresh backend, so no cache warmed above hides an unqualified reference.
\c
SET client_min_messages TO WARNING;
SET search_path TO '';
UPDATE app.tb_user SET name = 'alicia' WHERE pk_user = 1;
INSERT INTO app.tb_post (pk_post, fk_user, title) VALUES (3, 1, 'p3');
DELETE FROM app.tb_post WHERE pk_post = 2;
SELECT app.assert_136('user', 'auto-commit DML');
SELECT app.assert_136('post', 'auto-commit DML');

BEGIN;
UPDATE app.tb_post SET title = 'p1 edited' WHERE pk_post = 1;
UPDATE app.tb_user SET name = 'bobby' WHERE pk_user = 2;
COMMIT;
SELECT app.assert_136('post', 'explicit COMMIT');

SET pg_tviews.audit_enabled TO on;
UPDATE app.tb_user SET name = 'al' WHERE pk_user = 1;
RESET pg_tviews.audit_enabled;
SELECT app.assert_136('post', 'audited DML');

ALTER TABLE app.tb_post RENAME COLUMN title TO headline;
UPDATE app.tb_post SET headline = 'renamed' WHERE pk_post = 3;
SELECT app.assert_136('post', 'RENAME COLUMN');

CREATE TABLE app.unrelated (x int);
DROP TABLE app.unrelated;

CREATE FUNCTION app.tviews_triggers_on(tbl regclass) RETURNS bigint
LANGUAGE sql SET search_path = pg_catalog AS $$
    SELECT count(*) FROM pg_trigger t JOIN pg_proc p ON p.oid = t.tgfoid
    WHERE t.tgrelid = tbl AND p.pronamespace = 'tviews'::regnamespace $$;

DROP TABLE app.tv_post;
DO $$ BEGIN
    IF app.tviews_triggers_on('app.tb_post') <> 0 THEN
        RAISE EXCEPTION '#136 FAIL: DROP TABLE tv_post left triggers on tb_post';
    END IF;
    IF app.tviews_triggers_on('app.tb_user') <> 2 THEN
        RAISE EXCEPTION '#136 FAIL: tb_user should keep only tv_user''s two triggers, has %',
            app.tviews_triggers_on('app.tb_user');
    END IF;
END $$;
SELECT tviews.pg_tviews_drop('user');
DO $$ BEGIN
    IF app.tviews_triggers_on('app.tb_user') <> 0 THEN
        RAISE EXCEPTION '#136 FAIL: pg_tviews_drop left triggers on tb_user';
    END IF;
END $$;

-- 5. Trigger names: entities whose names share their first 53 characters get
--    distinct triggers on a shared table (PostgreSQL would truncate both names to
--    the same 63 bytes) ...
SET search_path TO app;
CREATE TABLE tb_invoice_line_adjustment_with_a_deliberately_long_name_a (
    pk_invoice_line_adjustment_with_a_deliberately_long_name_a int PRIMARY KEY,
    id    uuid NOT NULL DEFAULT gen_random_uuid(),
    label text NOT NULL
);
CREATE TABLE tb_invoice_line_adjustment_with_a_deliberately_long_name_b (
    pk_invoice_line_adjustment_with_a_deliberately_long_name_b int PRIMARY KEY,
    id    uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_invoice_line_adjustment_with_a_deliberately_long_name_a int NOT NULL
        REFERENCES tb_invoice_line_adjustment_with_a_deliberately_long_name_a
);
INSERT INTO tb_invoice_line_adjustment_with_a_deliberately_long_name_a VALUES (1, DEFAULT, 'x');
INSERT INTO tb_invoice_line_adjustment_with_a_deliberately_long_name_b VALUES (1, DEFAULT, 1);
SELECT tviews.pg_tviews_create('tv_invoice_line_adjustment_with_a_deliberately_long_name_a', $$
    SELECT pk_invoice_line_adjustment_with_a_deliberately_long_name_a, id,
           jsonb_build_object('label', label) AS data
    FROM app.tb_invoice_line_adjustment_with_a_deliberately_long_name_a $$);
SELECT tviews.pg_tviews_create('tv_invoice_line_adjustment_with_a_deliberately_long_name_b', $$
    SELECT b.pk_invoice_line_adjustment_with_a_deliberately_long_name_b, b.id,
           b.fk_invoice_line_adjustment_with_a_deliberately_long_name_a,
           jsonb_build_object('label', a.label) AS data
    FROM app.tb_invoice_line_adjustment_with_a_deliberately_long_name_b b
    JOIN app.tb_invoice_line_adjustment_with_a_deliberately_long_name_a a
      ON a.pk_invoice_line_adjustment_with_a_deliberately_long_name_a
       = b.fk_invoice_line_adjustment_with_a_deliberately_long_name_a $$);
DO $$ BEGIN
    IF app.tviews_triggers_on('app.tb_invoice_line_adjustment_with_a_deliberately_long_name_a') <> 4 THEN
        RAISE EXCEPTION '#136 FAIL: expected two triggers per TVIEW on the shared table, got %',
            app.tviews_triggers_on('app.tb_invoice_line_adjustment_with_a_deliberately_long_name_a');
    END IF;
END $$;
UPDATE tb_invoice_line_adjustment_with_a_deliberately_long_name_a SET label = 'y';
DO $$ BEGIN
    IF (SELECT data->>'label' FROM tv_invoice_line_adjustment_with_a_deliberately_long_name_b)
       IS DISTINCT FROM 'y' THEN
        RAISE EXCEPTION '#136 FAIL: long-named TVIEW not refreshed through the shared table';
    END IF;
END $$;
SELECT tviews.pg_tviews_drop('invoice_line_adjustment_with_a_deliberately_long_name_b');
DO $$ BEGIN
    IF app.tviews_triggers_on('app.tb_invoice_line_adjustment_with_a_deliberately_long_name_a') <> 2 THEN
        RAISE EXCEPTION '#136 FAIL: dropping one long-named TVIEW should leave the other''s two triggers';
    END IF;
END $$;

-- ... and a TVIEW in a schema whose name is long and multibyte (PostgreSQL
-- truncates identifiers by bytes) leaves no trigger behind when dropped.
CREATE SCHEMA "schéma_très_long_pour_les_noms_de_déclencheurs";
SET search_path TO "schéma_très_long_pour_les_noms_de_déclencheurs";
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), body text);
INSERT INTO tb_note VALUES (1, DEFAULT, 'n');
SELECT tviews.pg_tviews_create('tv_note', $$
    SELECT pk_note, id, jsonb_build_object('body', body) AS data
    FROM "schéma_très_long_pour_les_noms_de_déclencheurs".tb_note $$);
UPDATE tb_note SET body = 'm';
DO $$ BEGIN
    IF (SELECT data->>'body' FROM tv_note) IS DISTINCT FROM 'm' THEN
        RAISE EXCEPTION '#136 FAIL: TVIEW in a multibyte schema not refreshed';
    END IF;
END $$;
SELECT tviews.pg_tviews_drop('note');
DO $$ BEGIN
    IF app.tviews_triggers_on('"schéma_très_long_pour_les_noms_de_déclencheurs".tb_note') <> 0 THEN
        RAISE EXCEPTION '#136 FAIL: dropping a TVIEW in a multibyte schema left its triggers';
    END IF;
END $$;

-- Entity flush_y's row trigger and entity y's flush trigger on a shared table
-- get different names.
SET search_path TO app;
CREATE TABLE tb_y (pk_y int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v text);
CREATE TABLE tb_flush_y (
    pk_flush_y int PRIMARY KEY,
    id         uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_y       int NOT NULL REFERENCES tb_y
);
INSERT INTO tb_y VALUES (1, DEFAULT, 'a');
INSERT INTO tb_flush_y VALUES (1, DEFAULT, 1);
SELECT tviews.pg_tviews_create('tv_y', $$
    SELECT pk_y, id, jsonb_build_object('v', v) AS data FROM app.tb_y $$);
SELECT tviews.pg_tviews_create('tv_flush_y', $$
    SELECT f.pk_flush_y, f.id, f.fk_y, jsonb_build_object('v', y.v) AS data
    FROM app.tb_flush_y f JOIN app.tb_y y ON y.pk_y = f.fk_y $$);
DO $$ BEGIN
    IF app.tviews_triggers_on('app.tb_y') <> 4 THEN
        RAISE EXCEPTION '#136 FAIL: tv_y and tv_flush_y should each have two triggers on tb_y';
    END IF;
END $$;

-- A schema whose name contains ':' (the table name lookup used to split on it).
CREATE SCHEMA "app:v2";
CREATE TABLE "app:v2".tb_z (pk_z int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v text);
SET search_path TO "app:v2";
SELECT tviews.pg_tviews_create('tv_z', $$
    SELECT pk_z, id, jsonb_build_object('v', v) AS data FROM "app:v2".tb_z $$);
SELECT tviews.pg_tviews_drop('z');
DO $$ BEGIN
    IF app.tviews_triggers_on('"app:v2".tb_z') <> 0 THEN
        RAISE EXCEPTION '#136 FAIL: dropping a TVIEW in schema "app:v2" left its triggers';
    END IF;
END $$;

RESET search_path;
DROP SCHEMA "schéma_très_long_pour_les_noms_de_déclencheurs" CASCADE;
DROP SCHEMA "app:v2" CASCADE;
DROP SCHEMA app CASCADE;
DROP EXTENSION pg_tviews CASCADE;

SELECT 'issue #136 tviews schema: PASS' AS result;
-- expect-output: issue #136 tviews schema: PASS

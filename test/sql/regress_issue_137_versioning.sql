-- Regression test (#137): versioned extension SQL, catalog revision guard, and
-- re-registration.
--
-- The extension SQL said 0.1.0 in every release, a library could run against any
-- catalog, and a TVIEW registered by an older release kept its old metadata until
-- it was dropped and re-created. Now the extension version is the release, the
-- library refuses a catalog of another revision (with the command that fixes it),
-- and pg_tviews_reregister[_all]() re-derives a TVIEW's metadata and triggers in
-- place, clearing needs_reregister.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_137_versioning.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP ROLE IF EXISTS regress_137_other;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- 1. The extension version is the release.
DO $$ BEGIN
    IF (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews')
       IS DISTINCT FROM tviews.pg_tviews_version() THEN
        RAISE EXCEPTION '#137 FAIL: extversion % is not the release %',
            (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews'),
            tviews.pg_tviews_version();
    END IF;
END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES tb_user,
    title   text
);
CREATE TABLE tb_order (
    pk_order int PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user  int NOT NULL REFERENCES tb_user,
    total    numeric
);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (1, 1, 10);

SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
SELECT pg_tviews_create_aggregate('tv_user_orders', $$
    SELECT o.fk_user AS pk_user_orders, u.id, jsonb_build_object('orders', count(*)) AS data
    FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id
$$, '{"tb_order": "fk_user", "tb_user": "pk_user"}');

CREATE FUNCTION assert_137(step text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE entity text; d bigint;
BEGIN
    FOREACH entity IN ARRAY ARRAY['user', 'post', 'user_orders'] LOOP
        EXECUTE format(
            'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tviews.public__tv_%1$s)
                         UNION ALL (SELECT pk_%1$s, data FROM tviews.public__tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d',
            entity) INTO d;
        IF d <> 0 THEN
            RAISE EXCEPTION '#137 FAIL after %: tv_% diverges from v_% (% rows)',
                step, entity, entity, d;
        END IF;
    END LOOP;
END $$;

-- 2. The library refuses a catalog of another revision, in a fresh backend, and
--    passes once the catalog matches again.
CREATE TABLE saved_revision AS SELECT tviews.pg_tviews_catalog_revision() AS r;
CREATE FUNCTION set_revision(r int) RETURNS void LANGUAGE plpgsql AS $f$
BEGIN
    EXECUTE format('CREATE OR REPLACE FUNCTION tviews.pg_tviews_catalog_revision()
                    RETURNS integer LANGUAGE sql IMMUTABLE AS %L', 'SELECT ' || r);
END $f$;
-- The error and hint a base-table write raises, or NULL.
CREATE FUNCTION write_error() RETURNS text LANGUAGE plpgsql AS $f$
DECLARE msg text; hint text;
BEGIN
    UPDATE tb_user SET name = 'alicia' WHERE pk_user = 1;
    RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, hint = PG_EXCEPTION_HINT;
    RETURN msg || ' / ' || hint;
END $f$;

-- An older catalog: update the extension.
SELECT set_revision(0);
\c
SET client_min_messages TO WARNING;
DO $$ BEGIN
    IF coalesce(write_error(), '') NOT LIKE
       '%library catalog revision%does not match the installed extension (0)%ALTER EXTENSION pg_tviews UPDATE%' THEN
        RAISE EXCEPTION '#137 FAIL: older catalog not refused: %', write_error();
    END IF;
END $$;
-- A newer catalog: install the matching library.
SELECT set_revision(999);
\c
SET client_min_messages TO WARNING;
DO $$ BEGIN
    IF coalesce(write_error(), '') NOT LIKE
       '%does not match the installed extension (999)%newer than this library%' THEN
        RAISE EXCEPTION '#137 FAIL: newer catalog not refused: %', write_error();
    END IF;
END $$;
-- Restored in the same session: only a match is remembered, so the write passes.
DO $$ BEGIN
    EXECUTE format('CREATE OR REPLACE FUNCTION tviews.pg_tviews_catalog_revision()
                    RETURNS integer LANGUAGE sql IMMUTABLE AS %L',
                   'SELECT ' || (SELECT r FROM saved_revision));
END $$;
UPDATE tb_user SET name = 'alicia' WHERE pk_user = 1;
SELECT assert_137('revision restored');

-- A catalog without a revision (a 0.1.0 install) points to the migration script.
ALTER EXTENSION pg_tviews DROP FUNCTION tviews.pg_tviews_catalog_revision();
ALTER FUNCTION tviews.pg_tviews_catalog_revision() RENAME TO saved_catalog_revision;
\c
SET client_min_messages TO WARNING;
DO $$
DECLARE msg text; hint text;
BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_refresh('user');
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, hint = PG_EXCEPTION_HINT;
    END;
    IF msg IS NULL OR hint NOT LIKE '%scripts/migrate-from-0.1.0.sql%' THEN
        RAISE EXCEPTION '#137 FAIL: catalog without a revision not refused: % / %', msg, hint;
    END IF;
END $$;
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_health_check()
                   WHERE component = 'catalog' AND status = 'ERROR'
                     AND message LIKE '%migrate-from-0.1.0.sql%') THEN
        RAISE EXCEPTION '#137 FAIL: health check does not report a 0.1.0 catalog';
    END IF;
END $$;
ALTER FUNCTION tviews.saved_catalog_revision() RENAME TO pg_tviews_catalog_revision;
ALTER EXTENSION pg_tviews ADD FUNCTION tviews.pg_tviews_catalog_revision();

-- 3. A TVIEW registered by an older release: derived metadata missing and base-table
--    triggers gone (as after the 0.1.0 migration), plus a stray trigger. It does not
--    follow its base tables until it is re-registered.
CREATE TABLE meta_at_create AS
    SELECT entity, cascade_paths, fk_columns, dependency_types, dependency_paths,
           direct_map_columns, direct_map_keys, aggregate_embeds
    FROM tviews.pg_tview_meta;
CREATE TABLE triggers_at_create AS
    SELECT tgrelid, tgname, tgfoid, tgargs FROM pg_trigger
    WHERE tgfoid IN (SELECT oid FROM pg_proc WHERE pronamespace = 'tviews'::regnamespace);
UPDATE tviews.pg_tview_meta
   SET cascade_paths = '{}', direct_map_columns = '{}', direct_map_keys = '{}',
       aggregate_embeds = '{}', needs_reregister = true
 WHERE entity IN ('post', 'user_orders');
DO $$
DECLARE t record;
BEGIN
    FOR t IN SELECT tr.tgname, tr.tgrelid::regclass AS rel
             FROM pg_trigger tr JOIN pg_proc p ON p.oid = tr.tgfoid
             WHERE p.pronamespace = 'tviews'::regnamespace
               AND tr.tgargs IN (convert_to('post', 'UTF8') || '\x00'::bytea,
                                 convert_to('user_orders', 'UTF8') || '\x00'::bytea)
    LOOP
        EXECUTE format('DROP TRIGGER %I ON %s', t.tgname, t.rel);
    END LOOP;
END $$;
CREATE TABLE unrelated (x int);
CREATE TRIGGER trg_tview_stray AFTER INSERT ON unrelated
    FOR EACH ROW EXECUTE FUNCTION tviews.pg_tview_trigger_handler("post");

DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_health_check()
                   WHERE component = 'reregister' AND status = 'WARNING'
                     AND message LIKE '2 TVIEWs%pg_tviews_reregister_all()%') THEN
        RAISE EXCEPTION '#137 FAIL: health check does not report needs_reregister: %',
            (SELECT string_agg(component || '=' || message, '; ')
             FROM tviews.pg_tviews_health_check());
    END IF;
END $$;

-- A column rename re-derives metadata but installs no trigger: it keeps the flag.
ALTER TABLE tb_post RENAME COLUMN title TO headline;
ALTER TABLE tb_post RENAME COLUMN headline TO title;
DO $$ BEGIN
    IF NOT (SELECT needs_reregister FROM tviews.pg_tview_meta WHERE entity = 'post') THEN
        RAISE EXCEPTION '#137 FAIL: a column rename cleared needs_reregister';
    END IF;
END $$;

-- 4. Only the TVIEW's owner (or the extension owner) may re-register it.
CREATE ROLE regress_137_other;
GRANT USAGE ON SCHEMA public TO regress_137_other;
SET ROLE regress_137_other;
DO $$ BEGIN
    PERFORM tviews.pg_tviews_reregister('post');
    RAISE EXCEPTION '#137 FAIL: a role not owning tv_post re-registered it';
EXCEPTION WHEN insufficient_privilege THEN
    NULL;
END $$;
RESET ROLE;

-- 5. reregister_all(): dependencies first, per-entity status, flags cleared,
--    metadata and triggers exactly as at creation, the stray trigger gone.
CREATE TABLE reregistered AS
    SELECT * FROM tviews.pg_tviews_reregister_all() WITH ORDINALITY AS r(entity, status, n);
DO $$ BEGIN
    IF (SELECT count(*) FROM reregistered WHERE status = 'reregistered') <> 3 THEN
        RAISE EXCEPTION '#137 FAIL: reregister_all returned %',
            (SELECT string_agg(entity || ':' || status, ',' ORDER BY n) FROM reregistered);
    END IF;
    IF (SELECT n FROM reregistered WHERE entity = 'user')
       > (SELECT n FROM reregistered WHERE entity = 'post') THEN
        RAISE EXCEPTION '#137 FAIL: post re-registered before user, which it reads';
    END IF;
    IF EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE needs_reregister) THEN
        RAISE EXCEPTION '#137 FAIL: needs_reregister not cleared';
    END IF;
    IF EXISTS (SELECT entity, cascade_paths, fk_columns, dependency_types, dependency_paths,
                      direct_map_columns, direct_map_keys, aggregate_embeds
               FROM tviews.pg_tview_meta
               EXCEPT SELECT * FROM meta_at_create) THEN
        RAISE EXCEPTION '#137 FAIL: re-derived metadata differs from creation';
    END IF;
    IF EXISTS ((SELECT tgrelid, tgname, tgfoid, tgargs FROM pg_trigger
                WHERE tgfoid IN (SELECT oid FROM pg_proc
                                 WHERE pronamespace = 'tviews'::regnamespace)
                EXCEPT SELECT * FROM triggers_at_create)
               UNION ALL
               (SELECT * FROM triggers_at_create
                EXCEPT SELECT tgrelid, tgname, tgfoid, tgargs FROM pg_trigger)) THEN
        RAISE EXCEPTION '#137 FAIL: triggers after reregister_all differ from creation';
    END IF;
    IF EXISTS (SELECT 1 FROM tviews.pg_tviews_health_check()
               WHERE component IN ('triggers', 'reregister') AND status <> 'OK') THEN
        RAISE EXCEPTION '#137 FAIL: health check not clean after reregister_all';
    END IF;
END $$;
UPDATE tb_user SET name = 'al' WHERE pk_user = 1;
UPDATE tb_post SET title = 'p1!' WHERE pk_post = 1;
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (2, 2, 5);
SELECT assert_137('after reregister_all');

-- 6. A TVIEW that cannot be re-derived: its status is the error; strict raises.
UPDATE tviews.pg_tview_meta SET definition = 'SELECT nonsense FROM', needs_reregister = true
 WHERE entity = 'user_orders';
DO $$ BEGIN
    IF (SELECT status FROM tviews.pg_tviews_reregister_all() WHERE entity = 'user_orders')
       IN ('reregistered') THEN
        RAISE EXCEPTION '#137 FAIL: an invalid definition was reported re-registered';
    END IF;
    IF (SELECT status FROM tviews.pg_tviews_reregister_all() WHERE entity = 'post')
       <> 'reregistered' THEN
        RAISE EXCEPTION '#137 FAIL: one failing TVIEW stopped the others';
    END IF;
    BEGIN
        PERFORM tviews.pg_tviews_reregister_all(strict => true);
    EXCEPTION WHEN OTHERS THEN
        RETURN;
    END;
    RAISE EXCEPTION '#137 FAIL: reregister_all(strict => true) did not raise';
END $$;

DROP EXTENSION pg_tviews CASCADE;
DROP OWNED BY regress_137_other;
DROP ROLE regress_137_other;

SELECT 'issue #137 versioning: PASS' AS result;
-- expect-output: issue #137 versioning: PASS

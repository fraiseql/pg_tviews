-- Regression test: a write needs only DML on the written table and USAGE on its
-- schema and on tviews, whatever TVIEWs exist elsewhere.
--
-- #251 was one instance: a write loads the catalog rows of every TVIEW, and a
-- reference of a TVIEW in a schema the writer could not use was resolved by name,
-- which checks USAGE. This file puts a TVIEW of every kind the catalog knows in a
-- schema the writer has no grant on (`ph`), then runs every write-path entry point
-- from a fresh backend as a writer that holds only the privileges above on its
-- own schema (`pw`). One TVIEW in `ph` reads a `pw` table: the writer's write
-- cascades into it, and the refresh runs as that TVIEW's owner.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_write_privilege_matrix.sql
--
-- expect-output: regress_write_privilege_matrix: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS pw CASCADE;
DROP SCHEMA IF EXISTS ph CASCADE;
DO $$
DECLARE r text;
BEGIN
    FOREACH r IN ARRAY ARRAY['regress_pm_writer', 'regress_pm_hidden'] LOOP
        IF EXISTS (SELECT FROM pg_roles WHERE rolname = r) THEN
            EXECUTE format('DROP OWNED BY %I', r);
            EXECUTE format('DROP ROLE %I', r);
        END IF;
    END LOOP;
END $$;
CREATE ROLE regress_pm_writer;
CREATE ROLE regress_pm_hidden;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- The writer's schema: a TVIEW per base table, an array of children (a write to a
-- child cascades into its parent) and a parent column copied into its children
-- (fan-out).
CREATE SCHEMA pw;
CREATE TABLE pw.tb_author (
    pk_author int PRIMARY KEY,
    id        uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    name      text NOT NULL
);
CREATE TABLE pw.tb_book (
    pk_book   int PRIMARY KEY,
    id        uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_author int NOT NULL REFERENCES pw.tb_author,
    title     text NOT NULL
);
CREATE TABLE pw.tb_doc (
    pk_doc int PRIMARY KEY,
    id     uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    body   text NOT NULL
);
INSERT INTO pw.tb_author (pk_author, name) VALUES (1, 'ann'), (2, 'ben');
INSERT INTO pw.tb_book (pk_book, fk_author, title) VALUES (1, 1, 'b1'), (2, 1, 'b2'), (3, 2, 'b3');
INSERT INTO pw.tb_doc (pk_doc, body) VALUES (1, 'd1');

SELECT tviews.pg_tviews_create('pw.tv_author', $$
    SELECT a.pk_author, a.id,
           jsonb_build_object('name', a.name,
               'books', COALESCE(jsonb_agg(jsonb_build_object('title', b.title)
                   ORDER BY b.pk_book) FILTER (WHERE b.pk_book IS NOT NULL), '[]'::jsonb)) AS data
    FROM pw.tb_author a LEFT JOIN pw.tb_book b ON b.fk_author = a.pk_author
    GROUP BY a.pk_author, a.id, a.name $$);
SELECT tviews.pg_tviews_create('pw.tv_book', $$
    SELECT b.pk_book, b.id, b.fk_author,
           jsonb_build_object('title', b.title, 'author', a.name) AS data
    FROM pw.tb_book b JOIN pw.tb_author a ON a.pk_author = b.fk_author $$);
SELECT tviews.pg_tviews_create('pw.tv_doc', $$
    SELECT d.pk_doc, d.id, jsonb_build_object('body', d.body) AS data FROM pw.tb_doc d $$);

-- The hidden schema: one TVIEW of every kind, owned by regress_pm_hidden.
CREATE SCHEMA ph;
CREATE TABLE ph.tb_plain (
    pk_plain int PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    v        text NOT NULL
);
CREATE TABLE ph.tb_parent (
    pk_parent int PRIMARY KEY,
    id        uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    name      text NOT NULL
);
CREATE TABLE ph.tb_child (
    pk_child  int PRIMARY KEY,
    id        uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_parent int NOT NULL REFERENCES ph.tb_parent,
    n         int NOT NULL
);
CREATE TABLE ph.tb_grandchild (
    pk_grandchild int PRIMARY KEY,
    id            uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_child      int NOT NULL REFERENCES ph.tb_child,
    label         text NOT NULL
);
CREATE TABLE ph.tb_setting (key text PRIMARY KEY, value text NOT NULL);
CREATE TABLE ph.tb_event (
    pk_event int PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    at       date NOT NULL
);
INSERT INTO ph.tb_plain (pk_plain, v) VALUES (1, 'x');
INSERT INTO ph.tb_parent (pk_parent, name) VALUES (1, 'p');
INSERT INTO ph.tb_child (pk_child, fk_parent, n) VALUES (1, 1, 10), (2, 1, 20);
INSERT INTO ph.tb_grandchild (pk_grandchild, fk_child, label) VALUES (1, 1, 'g');
INSERT INTO ph.tb_setting VALUES ('suffix', '!');
INSERT INTO ph.tb_event (pk_event, at) VALUES (1, DATE '2026-01-01');
CREATE FUNCTION ph.suffix() RETURNS text LANGUAGE sql STABLE
    AS $$ SELECT value FROM ph.tb_setting WHERE key = 'suffix' $$;

-- Plain.
SELECT tviews.pg_tviews_create('ph.tv_plain', $$
    SELECT p.pk_plain, p.id, jsonb_build_object('v', p.v) AS data FROM ph.tb_plain p $$);
-- A reference (fk_parent) and a two-hop fan-out (the parent's name in each grandchild).
SELECT tviews.pg_tviews_create('ph.tv_parent', $$
    SELECT p.pk_parent, p.id, jsonb_build_object('name', p.name) AS data FROM ph.tb_parent p $$);
SELECT tviews.pg_tviews_create('ph.tv_child', $$
    SELECT c.pk_child, c.id, c.fk_parent, jsonb_build_object('n', c.n) AS data FROM ph.tb_child c $$);
SELECT tviews.pg_tviews_create('ph.tv_grandchild', $$
    SELECT g.pk_grandchild, g.id, g.fk_child,
           jsonb_build_object('label', g.label, 'parent', p.name) AS data
    FROM ph.tb_grandchild g JOIN ph.tb_child c ON c.pk_child = g.fk_child
    JOIN ph.tb_parent p ON p.pk_parent = c.fk_parent $$);
-- Aggregate.
SELECT tviews.pg_tviews_create('ph.tv_parent_stats', $$
    SELECT c.fk_parent AS pk_parent_stats, p.id, jsonb_build_object('total', sum(c.n)) AS data
    FROM ph.tb_child c JOIN ph.tb_parent p ON p.pk_parent = c.fk_parent
    GROUP BY c.fk_parent, p.id $$,
    '{"group_keys": {"tb_child": "fk_parent", "tb_parent": "pk_parent"}}');
-- A set operation.
SELECT tviews.pg_tviews_create('ph.tv_feed', $$
    SELECT c.pk_child AS pk_feed, c.fk_parent, jsonb_build_object('t', 'c') AS data FROM ph.tb_child c
    UNION ALL
    SELECT p.pk_parent + 1000, p.pk_parent, jsonb_build_object('t', 'p') FROM ph.tb_parent p $$);
-- A function that reads a table.
SELECT tviews.pg_tviews_create('ph.tv_labelled', $$
    SELECT p.pk_plain AS pk_labelled, p.id, jsonb_build_object('v', p.v || ph.suffix()) AS data
    FROM ph.tb_plain p $$,
    '{"function_reads": {"ph.suffix()": ["ph.tb_setting"]},
      "uncascaded_tables": {"ph.tb_setting": "full_refresh"}}');
-- Time-dependent.
SELECT tviews.pg_tviews_create('ph.tv_event', $$
    SELECT e.pk_event, e.id, jsonb_build_object('past', e.at < CURRENT_DATE) AS data
    FROM ph.tb_event e $$,
    '{"time_refresh": "external"}');
-- UNLOGGED, with managed indexes, storage options and a declared type name.
SELECT tviews.pg_tviews_create('ph.tv_stored', $$
    SELECT p.pk_plain AS pk_stored, p.id, jsonb_build_object('v', p.v) AS data
    FROM ph.tb_plain p $$,
    '{"logged": false, "indexes": [["v"]], "data_storage": "external",
      "toast_tuple_target": 3000, "fillfactor": 70, "typename": "HiddenStored"}');
-- Reads the writer's table: the writer's write cascades into it.
SELECT tviews.pg_tviews_create('ph.tv_mirror', $$
    SELECT d.pk_doc AS pk_mirror, d.id, jsonb_build_object('body', d.body) AS data
    FROM pw.tb_doc d $$);

DO $$
BEGIN
    IF (SELECT count(*) FROM tviews.registry WHERE schema = 'ph') <> 10 THEN
        RAISE EXCEPTION 'matrix setup: % TVIEWs in ph, want 10',
            (SELECT count(*) FROM tviews.registry WHERE schema = 'ph');
    END IF;
    IF (SELECT count(*) FROM tviews.pg_tview_references
        WHERE entity IN ('child', 'grandchild') AND chosen) <> 2 THEN
        RAISE EXCEPTION 'matrix setup: the ph references are not published';
    END IF;
END $$;

-- Ownership: regress_pm_hidden owns every ph TVIEW and reads what they read.
GRANT USAGE ON SCHEMA ph, pw TO regress_pm_hidden;
GRANT SELECT ON ALL TABLES IN SCHEMA ph TO regress_pm_hidden;
GRANT SELECT ON pw.tb_doc TO regress_pm_hidden;
DO $$
DECLARE r record;
BEGIN
    FOR r IN SELECT entity FROM tviews.registry WHERE schema = 'ph' LOOP
        EXECUTE format('ALTER TABLE ph.%I OWNER TO regress_pm_hidden', 'tv_' || r.entity);
        EXECUTE format('ALTER VIEW tviews.%I OWNER TO regress_pm_hidden', 'ph__tv_' || r.entity);
    END LOOP;
END $$;

-- The writer: DML on the pw base tables, USAGE on pw and tviews. Nothing on ph.
GRANT USAGE ON SCHEMA pw, tviews TO regress_pm_writer;
GRANT SELECT, INSERT, UPDATE, DELETE ON pw.tb_author, pw.tb_book, pw.tb_doc TO regress_pm_writer;

-- Every TVIEW equals its backing view (run as a superuser).
CREATE FUNCTION public.assert_pm(step text) RETURNS void
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE r record; d bigint;
BEGIN
    FOR r IN SELECT schema, entity FROM tviews.registry WHERE schema IN ('pw', 'ph') LOOP
        EXECUTE format(
            'SELECT count(*) FROM ((SELECT pk_%2$s, data FROM %1$I.%3$I
                                    EXCEPT SELECT pk_%2$s, data FROM tviews.%4$I)
                         UNION ALL (SELECT pk_%2$s, data FROM tviews.%4$I
                                    EXCEPT SELECT pk_%2$s, data FROM %1$I.%3$I)) d',
            r.schema, r.entity, 'tv_' || r.entity, r.schema || '__tv_' || r.entity) INTO d;
        IF d <> 0 THEN
            RAISE EXCEPTION 'matrix FAIL after %: %.tv_% diverges from its view (% rows)',
                step, r.schema, r.entity, d;
        END IF;
    END LOOP;
END $$;

-- 1. Direct writes, each from a fresh backend (the catalog cache is cold).
\c
SET SESSION AUTHORIZATION regress_pm_writer;
INSERT INTO pw.tb_author (pk_author, name) VALUES (3, 'cat');
\c
SET SESSION AUTHORIZATION regress_pm_writer;
UPDATE pw.tb_doc SET body = 'd1 edited' WHERE pk_doc = 1;      -- also ph.tv_mirror
\c
SET SESSION AUTHORIZATION regress_pm_writer;
DELETE FROM pw.tb_author WHERE pk_author = 3;
RESET SESSION AUTHORIZATION;
SELECT public.assert_pm('direct writes');
DO $$
BEGIN
    IF (SELECT data->>'body' FROM ph.tv_mirror WHERE pk_mirror = 1) IS DISTINCT FROM 'd1 edited' THEN
        RAISE EXCEPTION 'matrix FAIL: ph.tv_mirror was not refreshed: %',
            (SELECT data FROM ph.tv_mirror);
    END IF;
END $$;

-- 2. A child's write cascades into its parent; a parent's into its children.
\c
SET SESSION AUTHORIZATION regress_pm_writer;
INSERT INTO pw.tb_book (pk_book, fk_author, title) VALUES (4, 2, 'b4');
\c
SET SESSION AUTHORIZATION regress_pm_writer;
UPDATE pw.tb_author SET name = 'annie' WHERE pk_author = 1;
RESET SESSION AUTHORIZATION;
SELECT public.assert_pm('cascades');

-- 3. An explicit transaction, flushed by the COMMIT hook, with a report.
\c
SET SESSION AUTHORIZATION regress_pm_writer;
BEGIN;
UPDATE pw.tb_book SET title = 'b1 edited' WHERE pk_book = 1;
DELETE FROM pw.tb_book WHERE pk_book = 2;
SELECT tviews.pg_tviews_flush_and_report(include_data => false, reset => false) AS report \gset
-- The data is read as the caller: without SELECT on tv_book it is refused.
DO $$
BEGIN
    PERFORM tviews.pg_tviews_flush_and_report(reset => false);
    RAISE EXCEPTION 'matrix FAIL: the report read tv_book data without SELECT on it';
EXCEPTION WHEN insufficient_privilege THEN NULL;
END $$;
UPDATE pw.tb_doc SET body = 'd1 again' WHERE pk_doc = 1;
COMMIT;
RESET SESSION AUTHORIZATION;
SELECT public.assert_pm('explicit COMMIT');

-- 4. With publish_changes = 'rows': the feed and the report's collections.
ALTER DATABASE :"DBNAME" SET pg_tviews.publish_changes = 'rows';
\c
SET SESSION AUTHORIZATION regress_pm_writer;
BEGIN;
INSERT INTO pw.tb_book (pk_book, fk_author, title) VALUES (5, 1, 'b5');
SELECT tviews.pg_tviews_flush_and_report(include_data => false) AS report \gset
COMMIT;
UPDATE pw.tb_book SET fk_author = 2 WHERE pk_book = 5;
SELECT tviews.pg_tviews_feed_gaps() >= 0 AS gaps_readable \gset
SELECT count(*) > 0 AS registry_readable FROM tviews.registry \gset
RESET SESSION AUTHORIZATION;
ALTER DATABASE :"DBNAME" RESET pg_tviews.publish_changes;
SELECT public.assert_pm('publish_changes rows');
SELECT :'report'::jsonb ? 'collections' AS has_collections \gset
\if :has_collections
\else
    \echo 'matrix FAIL: the report has no collections:' :'report'
    SELECT 1/0;
\endif
\if :gaps_readable
\else
    \echo 'matrix FAIL: pg_tviews_feed_gaps()'
    SELECT 1/0;
\endif
\if :registry_readable
\else
    \echo 'matrix FAIL: tviews.registry is empty for the writer'
    SELECT 1/0;
\endif

-- 5. A catalog change while the writer's backend has the catalog cached: the next
--    write reloads it as the writer.
\c
SET SESSION AUTHORIZATION regress_pm_writer;
UPDATE pw.tb_author SET name = 'ann' WHERE pk_author = 1;
RESET SESSION AUTHORIZATION;
SELECT tviews.pg_tviews_reregister('ph.tv_child');
SET SESSION AUTHORIZATION regress_pm_writer;
UPDATE pw.tb_book SET title = 'b3 edited' WHERE pk_book = 3;
UPDATE pw.tb_doc SET body = 'd1 last' WHERE pk_doc = 1;
RESET SESSION AUTHORIZATION;
SELECT public.assert_pm('after a catalog change');

SET client_min_messages TO WARNING;
DROP SCHEMA pw CASCADE;
DROP SCHEMA ph CASCADE;
DROP FUNCTION public.assert_pm(text);
DROP OWNED BY regress_pm_writer;
DROP ROLE regress_pm_writer;
DROP OWNED BY regress_pm_hidden;
DROP ROLE regress_pm_hidden;

\echo 'regress_write_privilege_matrix: PASS'

-- Regression test (#136): a database with pg_tviews is usable by ordinary roles.
--
-- Everything pg_tviews did ran as the current role, so only the extension owner
-- could write to a base table: the row trigger could not read pg_tview_meta, the
-- refresh wrote tv_* as the writer, the audit flush inserted as the writer, and a
-- DROP ... CASCADE that took a TVIEW's backing view ran the cleanup as the dropper.
-- Now the refresh runs as each TVIEW's owner (as REFRESH MATERIALIZED VIEW does,
-- with a safe search_path), the catalog is readable by everyone, and the drop
-- handler and audit writer are SECURITY DEFINER. The writer below only has DML on
-- the tb_* tables; the TVIEWs belong to another role.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_136_privileges.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS app CASCADE;
DROP ROLE IF EXISTS regress_136_writer;
DROP ROLE IF EXISTS regress_136_owner;
CREATE ROLE regress_136_owner;
CREATE ROLE regress_136_writer;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE SCHEMA app;
SET search_path TO app, public, tviews;
CREATE TABLE tb_user (
    pk_user int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL,
    bio     text
);
CREATE TABLE tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES tb_user,
    title   text NOT NULL
);
CREATE TABLE tb_thread (
    pk_thread int PRIMARY KEY,
    id        uuid NOT NULL DEFAULT gen_random_uuid(),
    title     text NOT NULL
);
CREATE TABLE tb_comment (
    pk_comment int PRIMARY KEY,
    id         uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_thread  int NOT NULL REFERENCES tb_thread,
    body       text NOT NULL
);
CREATE TABLE tb_order (
    pk_order int PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user  int NOT NULL REFERENCES tb_user,
    total    numeric NOT NULL
);
CREATE TABLE tb_tag (
    pk_tag  int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_post int NOT NULL REFERENCES tb_post,
    label   text NOT NULL
);
INSERT INTO tb_user (pk_user, name, bio) VALUES (1, 'alice', 'a'), (2, 'bob', 'b');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 1, 'p2'), (3, 2, 'p3');
INSERT INTO tb_thread (pk_thread, title) VALUES (1, 't1');
INSERT INTO tb_comment (pk_comment, fk_thread, body) VALUES (1, 1, 'c1');
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (1, 1, 10), (2, 2, 5);
INSERT INTO tb_tag (pk_tag, fk_post, label) VALUES (1, 1, 't1'), (2, 3, 't2');

-- Own scalar columns: direct patch.
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data
    FROM app.tb_user $$);
-- A parent column copied into every child: fan-out patch.
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author_name', u.name) AS data
    FROM app.tb_post p JOIN app.tb_user u ON u.pk_user = p.fk_user $$);
-- Two hops from tb_user, through tb_post and tb_tag, which the writer cannot read.
SELECT pg_tviews_create('tv_tag', $$
    SELECT t.pk_tag, t.id, t.fk_post,
           jsonb_build_object('label', t.label, 'author', u.name) AS data
    FROM app.tb_tag t JOIN app.tb_post p ON p.pk_post = t.fk_post
    JOIN app.tb_user u ON u.pk_user = p.fk_user $$);
-- Children aggregated into an array.
SELECT pg_tviews_create('tv_thread', $$
    SELECT t.pk_thread, t.id,
           jsonb_build_object('title', t.title,
               'comments', COALESCE(jsonb_agg(jsonb_build_object('body', c.body)
                   ORDER BY c.pk_comment) FILTER (WHERE c.pk_comment IS NOT NULL),
                   '[]'::jsonb)) AS data
    FROM app.tb_thread t LEFT JOIN app.tb_comment c ON c.fk_thread = t.pk_thread
    GROUP BY t.pk_thread, t.id, t.title $$);
-- An aggregate TVIEW.
SELECT pg_tviews_create_aggregate('tv_user_orders', $$
    SELECT o.fk_user AS pk_user_orders, u.id,
           jsonb_build_object('orders', count(*), 'total', sum(o.total)) AS data
    FROM app.tb_order o JOIN app.tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id
$$, '{"tb_order": "fk_user", "tb_user": "pk_user"}');

-- The TVIEWs belong to regress_136_owner, which reads the base tables. The writer
-- gets DML on the base tables and nothing else.
GRANT USAGE ON SCHEMA app TO regress_136_owner, regress_136_writer;
GRANT CREATE ON SCHEMA app TO regress_136_writer;
GRANT SELECT ON tb_user, tb_post, tb_thread, tb_comment, tb_order, tb_tag
    TO regress_136_owner;
GRANT SELECT, INSERT, UPDATE, DELETE ON tb_user, tb_post, tb_thread, tb_comment, tb_order
    TO regress_136_writer;
ALTER TABLE tv_user OWNER TO regress_136_owner;
ALTER VIEW v_user OWNER TO regress_136_owner;
ALTER TABLE tv_post OWNER TO regress_136_owner;
ALTER VIEW v_post OWNER TO regress_136_owner;
ALTER TABLE tv_thread OWNER TO regress_136_owner;
ALTER VIEW v_thread OWNER TO regress_136_owner;
ALTER TABLE tv_tag OWNER TO regress_136_owner;
ALTER VIEW v_tag OWNER TO regress_136_owner;
ALTER TABLE tv_user_orders OWNER TO regress_136_owner;
ALTER VIEW v_user_orders OWNER TO regress_136_owner;

CREATE FUNCTION public.assert_136(step text) RETURNS void
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE entity text; d bigint;
BEGIN
    FOREACH entity IN ARRAY ARRAY['user', 'post', 'tag', 'thread', 'user_orders'] LOOP
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
    END LOOP;
END $$;
GRANT EXECUTE ON FUNCTION public.assert_136(text) TO PUBLIC;

-- 1. The writer's DML refreshes every kind of TVIEW, in auto-commit statements and
--    at an explicit COMMIT.
SET SESSION AUTHORIZATION regress_136_writer;
SET search_path TO app, public, tviews;
UPDATE tb_user SET bio = 'a2' WHERE pk_user = 1;                   -- direct patch
UPDATE tb_user SET name = 'alicia' WHERE pk_user = 1;              -- fan-out, hops
INSERT INTO tb_user (pk_user, name) VALUES (3, 'carol');           -- full row
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (4, 3, 'p4');
DELETE FROM tb_post WHERE pk_post = 2;
INSERT INTO tb_comment (pk_comment, fk_thread, body) VALUES (2, 1, 'c2');  -- array
UPDATE tb_comment SET body = 'c1 edited' WHERE pk_comment = 1;
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (3, 1, 7);  -- aggregate
BEGIN;
UPDATE tb_user SET name = 'bobby' WHERE pk_user = 2;
UPDATE tb_order SET total = 6 WHERE pk_order = 2;
DELETE FROM tb_comment WHERE pk_comment = 2;
COMMIT;
RESET SESSION AUTHORIZATION;
SELECT public.assert_136('writer DML');

-- 2. The refresh ignores the writer's search_path: a function planted by the
--    writer that is a better match than pg_catalog's must not run as the owner.
SET SESSION AUTHORIZATION regress_136_writer;
CREATE FUNCTION app.to_jsonb(app.tv_user) RETURNS jsonb LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '#136 FAIL: writer function ran inside the refresh as %', current_user;
END $$;
SET search_path TO app, pg_catalog;
INSERT INTO tb_user (pk_user, name) VALUES (4, 'dave');
DELETE FROM tb_user WHERE pk_user = 4;                  -- RETURNING to_jsonb(tv_user.*)
UPDATE tb_post SET title = 'p1 edited' WHERE pk_post = 1;
SET search_path TO app, public, tviews;
DROP FUNCTION app.to_jsonb(app.tv_user);
RESET SESSION AUTHORIZATION;
SELECT public.assert_136('writer search_path');

-- 3. Auditing: rows are written for the writer, who cannot read them.
ALTER DATABASE :"DBNAME" SET pg_tviews.audit_enabled = on;
\c
SET client_min_messages TO WARNING;
SET SESSION AUTHORIZATION regress_136_writer;
SET search_path TO app, public, tviews;
UPDATE tb_user SET name = 'al' WHERE pk_user = 1;
DO $$ BEGIN
    PERFORM count(*) FROM tviews.pg_tview_audit_log;
    RAISE EXCEPTION '#136 FAIL: the writer can read the audit log';
EXCEPTION WHEN insufficient_privilege THEN
    NULL;
END $$;
RESET SESSION AUTHORIZATION;
ALTER DATABASE :"DBNAME" RESET pg_tviews.audit_enabled;
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM tviews.pg_tview_audit_log
                   WHERE operation = 'REFRESH' AND performed_by = 'regress_136_writer')
       OR EXISTS (SELECT 1 FROM tviews.pg_tview_audit_log
                  WHERE performed_by <> 'regress_136_writer') THEN
        RAISE EXCEPTION '#136 FAIL: audit rows not attributed to the writer: %',
            (SELECT string_agg(DISTINCT performed_by, ', ') FROM tviews.pg_tview_audit_log);
    END IF;
END $$;
SELECT public.assert_136('audited DML');

-- 4. The writer reads the catalog, runs unrelated DDL, and cannot edit TVIEW rows.
SET SESSION AUTHORIZATION regress_136_writer;
SET search_path TO app, public, tviews;
DO $$ BEGIN
    IF (SELECT count(*) FROM tviews.pg_tview_meta) <> 5 THEN
        RAISE EXCEPTION '#136 FAIL: the writer does not see the five TVIEWs';
    END IF;
END $$;
CREATE TABLE app.unrelated (x int);
DROP TABLE app.unrelated;
DO $$ BEGIN
    UPDATE app.tv_user SET data = '{}' WHERE pk_user = 1;
    RAISE EXCEPTION '#136 FAIL: the writer could update a tv_* row';
EXCEPTION WHEN insufficient_privilege THEN
    NULL;
END $$;

-- 5. A DROP ... CASCADE by the writer takes the backing view of a TVIEW another
--    role owns: the DROP succeeds and the TVIEW is deregistered.
CREATE TABLE app.tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), body text);
INSERT INTO app.tb_note (pk_note, body) VALUES (1, 'n');
GRANT SELECT ON app.tb_note TO regress_136_owner;
RESET SESSION AUTHORIZATION;
SELECT pg_tviews_create('tv_note', $$
    SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM app.tb_note $$);
ALTER TABLE app.tv_note OWNER TO regress_136_owner;
ALTER VIEW app.v_note OWNER TO regress_136_owner;
SET SESSION AUTHORIZATION regress_136_writer;
SET search_path TO app, public, tviews;
UPDATE app.tb_note SET body = 'm';
DROP TABLE app.tb_note CASCADE;
RESET SESSION AUTHORIZATION;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'note')
       OR to_regclass('app.tv_note') IS NOT NULL THEN
        RAISE EXCEPTION '#136 FAIL: tv_note still registered after its base table was dropped';
    END IF;
END $$;
SELECT public.assert_136('after DROP CASCADE');

RESET search_path;
DROP SCHEMA app CASCADE;
DROP FUNCTION public.assert_136(text);
DROP EXTENSION pg_tviews CASCADE;
DROP ROLE regress_136_writer;
DROP ROLE regress_136_owner;

SELECT 'issue #136 privileges: PASS' AS result;
-- expect-output: issue #136 privileges: PASS

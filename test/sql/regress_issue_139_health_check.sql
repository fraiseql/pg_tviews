-- Regression test (#139): pg_tviews_health_check() checks pg_tviews' own triggers.
--
-- The orphaned-trigger check matched `tview_%`, which no pg_tviews trigger is named,
-- and resolved each TVIEW's base table as ('tb_' || entity)::regclass: wrong for an
-- aggregate TVIEW and for any TVIEW reading more than tb_<entity>, and an ERROR for
-- one off the search_path. Now a trigger is pg_tviews' when it calls one of its
-- trigger functions, and orphaned when the entity it carries is not registered or
-- does not read its table.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_139_health_check.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS app CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE SCHEMA app;
CREATE TABLE app.tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE app.tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES app.tb_user,
    title   text
);
CREATE TABLE app.tb_order (
    pk_order int PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user  int NOT NULL REFERENCES app.tb_user,
    total    numeric
);
INSERT INTO app.tb_user (pk_user, name) VALUES (1, 'alice');
INSERT INTO app.tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
INSERT INTO app.tb_order (pk_order, fk_user, total) VALUES (1, 1, 10);

-- Off the search_path, reading two tables, and an aggregate without a tb_<entity>.
SET search_path TO app, public, tviews;
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM app.tb_post p JOIN app.tb_user u ON u.pk_user = p.fk_user $$);
SELECT pg_tviews_create_aggregate('tv_user_orders', $$
    SELECT o.fk_user AS pk_user_orders, u.id, jsonb_build_object('orders', count(*)) AS data
    FROM app.tb_order o JOIN app.tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id
$$, '{"tb_order": "fk_user", "tb_user": "pk_user"}');
RESET search_path;

-- An unrelated trigger whose name starts with tview_.
CREATE TABLE public.audit_me (x int);
CREATE FUNCTION public.noop() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$;
CREATE TRIGGER tview_orphan AFTER INSERT ON public.audit_me
    FOR EACH ROW EXECUTE FUNCTION public.noop();

CREATE FUNCTION public.trigger_check() RETURNS text LANGUAGE sql AS $$
    SELECT status || ': ' || message FROM tviews.pg_tviews_health_check()
    WHERE component = 'triggers' $$;

-- 1. Healthy: the call succeeds, and every pg_tviews trigger is accounted for.
DO $$ BEGIN
    IF public.trigger_check() IS DISTINCT FROM 'OK: All triggers properly linked' THEN
        RAISE EXCEPTION '#139 FAIL: healthy database reported %', public.trigger_check();
    END IF;
END $$;

-- A TVIEW over a partitioned table: PostgreSQL copies its row trigger onto each
-- partition, and those copies are not orphans.
CREATE TABLE app.tb_event (
    pk_event int NOT NULL,
    id       uuid NOT NULL DEFAULT gen_random_uuid(),
    kind     text NOT NULL,
    PRIMARY KEY (pk_event, kind)
) PARTITION BY LIST (kind);
CREATE TABLE app.tb_event_a PARTITION OF app.tb_event FOR VALUES IN ('a');
CREATE TABLE app.tb_event_b PARTITION OF app.tb_event FOR VALUES IN ('b');
INSERT INTO app.tb_event (pk_event, kind) VALUES (1, 'a'), (2, 'b');
SET search_path TO app, public, tviews;
SELECT pg_tviews_create('tv_event', $$
    SELECT pk_event, id, jsonb_build_object('kind', kind) AS data FROM app.tb_event $$);
RESET search_path;
DO $$ BEGIN
    IF public.trigger_check() IS DISTINCT FROM 'OK: All triggers properly linked' THEN
        RAISE EXCEPTION '#139 FAIL: partitioned base table reported %', public.trigger_check();
    END IF;
END $$;

-- 2. A pg_tviews trigger on a table no TVIEW reads is orphaned.
CREATE TRIGGER trg_tview_planted AFTER INSERT ON public.audit_me
    FOR EACH ROW EXECUTE FUNCTION tviews.pg_tview_trigger_handler("post");
DO $$ BEGIN
    IF public.trigger_check() NOT LIKE 'WARNING: 1 orphaned trigger%trg_tview_planted%' THEN
        RAISE EXCEPTION '#139 FAIL: planted trigger not reported: %', public.trigger_check();
    END IF;
END $$;
DROP TRIGGER trg_tview_planted ON public.audit_me;

-- A dropped pg_tviews trigger is reported missing; one without an entity (as
-- older releases installed them) is reported for re-registration.
DO $$
DECLARE t record;
BEGIN
    SELECT tr.tgname, tr.tgrelid::regclass AS rel INTO t
    FROM pg_trigger tr JOIN pg_proc p ON p.oid = tr.tgfoid
    WHERE p.proname = 'pg_tview_flush_trigger' AND tr.tgrelid = 'app.tb_order'::regclass;
    EXECUTE format('DROP TRIGGER %I ON %s', t.tgname, t.rel);
END $$;
DO $$ BEGIN
    IF public.trigger_check() NOT LIKE 'WARNING: 1 missing trigger%user_orders (pg_tview_flush_trigger) on app.tb_order%' THEN
        RAISE EXCEPTION '#139 FAIL: missing trigger not reported: %', public.trigger_check();
    END IF;
END $$;
CREATE TRIGGER trg_tview_untagged AFTER INSERT ON public.audit_me
    FOR EACH ROW EXECUTE FUNCTION tviews.pg_tview_trigger_handler();
DO $$ BEGIN
    IF public.trigger_check() NOT LIKE '%1 trigger without an entity%trg_tview_untagged%' THEN
        RAISE EXCEPTION '#139 FAIL: untagged trigger not reported: %', public.trigger_check();
    END IF;
END $$;
DROP TRIGGER trg_tview_untagged ON public.audit_me;
SET search_path TO app, public, tviews;
SELECT pg_tviews_drop('user_orders');
RESET search_path;

-- 3. The triggers of a TVIEW whose registration is gone are orphaned: two on each
--    of tb_post and tb_user.
DELETE FROM tviews.pg_tview_meta WHERE entity = 'post';
DO $$ BEGIN
    IF public.trigger_check() NOT LIKE 'WARNING: 4 orphaned triggers%' THEN
        RAISE EXCEPTION '#139 FAIL: deregistered TVIEW''s triggers not reported: %',
            public.trigger_check();
    END IF;
END $$;

DROP SCHEMA app CASCADE;
DROP TABLE public.audit_me;
DROP FUNCTION public.noop();
DROP FUNCTION public.trigger_check();
DROP EXTENSION pg_tviews CASCADE;

SELECT 'issue #139 health check: PASS' AS result;
-- expect-output: issue #139 health check: PASS

-- Least privilege on every pg_tviews entry point.
--
-- Maintenance functions that act on every TVIEW are not executable by PUBLIC: an
-- operator role is granted them. Every function that acts on one TVIEW requires
-- owning it (or the extension), as ALTER TABLE and REFRESH MATERIALIZED VIEW do,
-- whoever may execute it. Every function of the extension is one or the other,
-- or only reads: a new function must be classified here.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_security_surface.sql
-- expect-output: security surface: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP ROLE IF EXISTS regress_sec_owner;
DROP ROLE IF EXISTS regress_sec_outsider;
DROP ROLE IF EXISTS regress_sec_operator;
CREATE ROLE regress_sec_owner;
CREATE ROLE regress_sec_outsider;
CREATE ROLE regress_sec_operator;
GRANT CREATE ON SCHEMA public TO regress_sec_owner;

-- The SQLSTATE `sql` fails with, as the current role ('' when it succeeds).
CREATE FUNCTION public.sqlstate_of(sql text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE sql;
    RETURN '';
EXCEPTION WHEN OTHERS THEN
    RETURN SQLSTATE;
END $$;
CREATE FUNCTION public.must_refuse(calls text[], who text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    call text;
    state text;
BEGIN
    FOREACH call IN ARRAY calls LOOP
        state := public.sqlstate_of(call);
        IF state <> '42501' THEN
            RAISE EXCEPTION 'FAIL: % as % gave SQLSTATE "%", not 42501', call, who, state;
        END IF;
    END LOOP;
END $$;
CREATE FUNCTION public.must_allow(calls text[], who text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    call text;
    state text;
BEGIN
    FOREACH call IN ARRAY calls LOOP
        state := public.sqlstate_of(call);
        IF state <> '' THEN
            RAISE EXCEPTION 'FAIL: % as % failed with SQLSTATE %', call, who, state;
        END IF;
    END LOOP;
END $$;

SET ROLE regress_sec_owner;
CREATE TABLE public.tb_doc (pk_doc bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text);
INSERT INTO public.tb_doc (pk_doc, title) VALUES (1, 'a');
SELECT pg_tviews_create('public.tv_doc', $$
    SELECT pk_doc, id, jsonb_build_object('title', title) AS data FROM public.tb_doc
$$);
RESET ROLE;

-- ── Every function is classified ────────────────────────────────────────────
-- What acts on every TVIEW: revoked from PUBLIC.
CREATE TABLE public.maintenance (fn regprocedure);
INSERT INTO public.maintenance VALUES
    ('tviews.pg_tviews_refresh_all()'),
    ('tviews.pg_tviews_refresh_all_entities()'),
    ('tviews.pg_tviews_rebuild_all(boolean)'),
    ('tviews.pg_tviews_reregister_all(boolean)'),
    ('tviews.pg_tviews_set_logged(text, boolean)'),
    ('tviews.pg_tviews_ensure_propagation_indexes(text, boolean)'),
    ('tviews.pg_tviews_invalidate_caches(oid)'),
    ('tviews.pg_tviews_audit_write(jsonb)');
DO $$
DECLARE
    leaked text;
    unclassified text;
BEGIN
    SELECT string_agg(fn::text, ', ') INTO leaked FROM public.maintenance
     WHERE has_function_privilege('public', fn, 'EXECUTE');
    IF leaked IS NOT NULL THEN
        RAISE EXCEPTION 'FAIL: PUBLIC may execute %', leaked;
    END IF;
    -- Everything else PUBLIC may execute: it reads, acts on the caller's
    -- session, checks ownership of the one TVIEW it acts on, or is a trigger or
    -- event-trigger function.
    SELECT string_agg(p.oid::regprocedure::text, ', ' ORDER BY 1) INTO unclassified
      FROM pg_proc p
     WHERE p.pronamespace = 'tviews'::regnamespace AND p.prokind = 'f'
       AND p.oid NOT IN (SELECT fn FROM public.maintenance)
       AND p.oid NOT IN (SELECT f::regprocedure FROM unnest(ARRAY[
           -- reads
           'tviews.contract_version()', 'tviews.pg_tviews_version()',
           'tviews.pg_tviews_catalog_revision()', 'tviews.pg_tviews_check_jsonb_delta()',
           'tviews.pg_tviews_health_check()', 'tviews.pg_tviews_profile(text,bigint)',
           'tviews.pg_tviews_performance_stats()', 'tviews.pg_tviews_queue_stats()',
           'tviews.pg_tviews_debug_queue()', 'tviews.pg_tviews_replication_status()',
           'tviews.pg_tviews_is_replica_readable(text)', 'tviews.pg_tviews_mapping_query(text,oid)',
           'tviews.pg_tviews_read_set_queries(text,oid)',
           'tviews.pg_tviews_show_cascade_path(text)', 'tviews.pg_tviews_defines_view(oid,text)',
           -- the caller's session
           'tviews.pg_tviews_flush_and_report(integer,boolean,boolean)',
           'tviews.pg_tviews_suspend_triggers()', 'tviews.pg_tviews_resume_triggers()',
           'tviews.pg_tviews_is_suspended()', 'tviews.pg_tviews_suspended_entities()',
           -- one TVIEW, owner checked (creation: the schema's CREATE privilege)
           'tviews.pg_tviews_create(text,text)', 'tviews.pg_tviews_create_aggregate(text,text,jsonb)',
           'tviews.pg_tviews_create_or_replace(text,text,jsonb)',
           'tviews.pg_tviews_drop(text,boolean,boolean)', 'tviews.pg_tviews_refresh(text)',
           'tviews.pg_tviews_reregister(text)', 'tviews.pg_tviews_set_typename(text,text)',
           'tviews.pg_tviews_recover_after_crash(text)',
           'tviews.pg_tviews_refresh_time_dependent(text)',
           'tviews.pg_tviews_handle_dropped(text)',
           -- triggers and event triggers
           'tviews.pg_tview_trigger_handler()', 'tviews.pg_tview_delta_trigger()',
           'tviews.pg_tview_flush_trigger()', 'tviews.pg_tview_truncate_trigger()',
           'tviews.pg_tviews_handle_ddl_event()', 'tviews.pg_tviews_handle_drop_event()',
           'tviews.pg_tviews_meta_rebind()', 'tviews.pg_tviews_meta_changed()']) f);
    IF unclassified IS NOT NULL THEN
        RAISE EXCEPTION 'FAIL: functions not classified by this test: %', unclassified;
    END IF;
END $$;

-- ── A role that owns nothing ────────────────────────────────────────────────
SET ROLE regress_sec_outsider;
SELECT public.must_refuse(ARRAY[
    'SELECT tviews.pg_tviews_refresh_all()',
    'SELECT tviews.pg_tviews_refresh_all_entities()',
    'SELECT * FROM tviews.pg_tviews_rebuild_all(false)',
    'SELECT * FROM tviews.pg_tviews_rebuild_all(true)',
    'SELECT * FROM tviews.pg_tviews_reregister_all()',
    'SELECT tviews.pg_tviews_set_logged(''doc'', true)',
    'SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes()',
    'SELECT tviews.pg_tviews_invalidate_caches(''public.tb_doc''::regclass)',
    -- someone else's TVIEW
    'SELECT tviews.pg_tviews_refresh(''doc'')',
    'SELECT tviews.pg_tviews_reregister(''doc'')',
    'SELECT tviews.pg_tviews_recover_after_crash(''doc'')',
    'SELECT tviews.pg_tviews_set_typename(''doc'', ''Doc'')',
    'SELECT tviews.pg_tviews_drop(''public.tv_doc'')',
    'SELECT tviews.pg_tviews_create_or_replace(''public.tv_doc'', ''SELECT pk_doc, id, '
        'jsonb_build_object(''''title'''', title) AS data FROM public.tb_doc'')'
], 'a role owning nothing');
RESET ROLE;

-- ── An operator role, granted the maintenance functions ─────────────────────
-- (docs/user-guides/operators.md). Bulk maintenance runs each TVIEW's view as its
-- owner; acting on one TVIEW still requires owning it.
GRANT EXECUTE ON FUNCTION tviews.pg_tviews_refresh_all(), tviews.pg_tviews_refresh_all_entities(),
    tviews.pg_tviews_rebuild_all(boolean), tviews.pg_tviews_reregister_all(boolean),
    tviews.pg_tviews_set_logged(text, boolean),
    tviews.pg_tviews_ensure_propagation_indexes(text, boolean)
    TO regress_sec_operator;
SET ROLE regress_sec_operator;
SELECT public.must_refuse(ARRAY[
    'SELECT tviews.pg_tviews_set_logged(''doc'', true)',
    'SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes(''doc'')',
    'SELECT tviews.pg_tviews_refresh(''doc'')'
], 'an operator');
RESET ROLE;
-- The bulk rebuilds work for it, whatever it may read: a deploy or restore tool
-- refills emptied TVIEWs (each read as its owner).
TRUNCATE public.tv_doc;
SET ROLE regress_sec_operator;
SELECT public.must_allow(ARRAY[
    'SELECT * FROM tviews.pg_tviews_rebuild_all(true)',
    'SELECT * FROM tviews.pg_tviews_rebuild_all(false)',
    'SELECT tviews.pg_tviews_refresh_all()',
    'SELECT tviews.pg_tviews_refresh_all_entities()'
], 'an operator');
RESET ROLE;
DO $$
BEGIN
    IF (SELECT count(*) FROM public.tv_doc) <> 1 THEN
        RAISE EXCEPTION 'FAIL: the operator''s rebuild did not refill tv_doc';
    END IF;
END $$;

-- ── The owner ───────────────────────────────────────────────────────────────
SET ROLE regress_sec_owner;
SELECT public.must_allow(ARRAY[
    'SELECT tviews.pg_tviews_refresh(''doc'')',
    'SELECT tviews.pg_tviews_reregister(''doc'')',
    'SELECT tviews.pg_tviews_recover_after_crash(''doc'')',
    'SELECT tviews.pg_tviews_set_typename(''doc'', ''Doc'')'
], 'the owner');
SELECT public.must_refuse(ARRAY[
    'SELECT tviews.pg_tviews_refresh_all()',
    'SELECT * FROM tviews.pg_tviews_rebuild_all(true)'
], 'the owner without the grant');
UPDATE public.tb_doc SET title = 'b';
RESET ROLE;
DO $$
BEGIN
    IF (SELECT data->>'title' FROM public.tv_doc WHERE pk_doc = 1) <> 'b' THEN
        RAISE EXCEPTION 'FAIL: tv_doc stale';
    END IF;
END $$;

DROP TABLE public.maintenance;
DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_doc;
DROP OWNED BY regress_sec_owner, regress_sec_outsider, regress_sec_operator;
DROP ROLE regress_sec_owner, regress_sec_outsider, regress_sec_operator;

\echo 'security surface: PASS'

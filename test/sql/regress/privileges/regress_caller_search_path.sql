-- The catalog SQL pg_tviews runs on a caller's behalf resolves its operators in
-- pg_catalog, whatever the caller's search_path holds; the names the caller
-- writes still resolve under that path.
--
-- pg_tviews compares catalog values pg_catalog has no exact operator for (an oid
-- with a regclass, an oid with an integer), where PostgreSQL takes an exact
-- operator from any schema on the path over pg_catalog's implicit cast. Here such
-- operators, in another role's schema on the caller's path, record each call;
-- every entry point is called through that path, and none of them is called.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_caller_search_path.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS regress_csp CASCADE;
DROP TABLE IF EXISTS public.csp_ran, tb_csp_post, tb_csp_tag;
DROP FUNCTION IF EXISTS csp_tag();
DROP ROLE IF EXISTS regress_csp;
CREATE ROLE regress_csp;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'caller search_path FAIL: %', what; END IF; END $$;

-- Each call of the other role's operators, and the role it ran as.
CREATE TABLE public.csp_ran (who text, op text, step text);
GRANT INSERT ON public.csp_ran TO PUBLIC;

CREATE SCHEMA regress_csp AUTHORIZATION regress_csp;
GRANT USAGE ON SCHEMA regress_csp TO PUBLIC;
SET ROLE regress_csp;
CREATE FUNCTION regress_csp.oid_regclass(oid, regclass) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO public.csp_ran VALUES (current_user, 'oid = regclass', pg_catalog.current_setting('csp.step', true));
    RETURN $1 OPERATOR(pg_catalog.=) $2::pg_catalog.oid;
END $$;
CREATE FUNCTION regress_csp.regclass_oid(regclass, oid) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO public.csp_ran VALUES (current_user, 'regclass = oid', pg_catalog.current_setting('csp.step', true));
    RETURN $1::pg_catalog.oid OPERATOR(pg_catalog.=) $2;
END $$;
CREATE FUNCTION regress_csp.oid_int(oid, integer) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO public.csp_ran VALUES (current_user, 'oid = integer', pg_catalog.current_setting('csp.step', true));
    RETURN $1 OPERATOR(pg_catalog.=) $2::pg_catalog.oid;
END $$;
CREATE OPERATOR regress_csp.= (LEFTARG = oid, RIGHTARG = regclass, FUNCTION = regress_csp.oid_regclass);
CREATE OPERATOR regress_csp.= (LEFTARG = regclass, RIGHTARG = oid, FUNCTION = regress_csp.regclass_oid);
CREATE OPERATOR regress_csp.= (LEFTARG = oid, RIGHTARG = integer, FUNCTION = regress_csp.oid_int);
RESET ROLE;

CREATE TABLE tb_csp_tag (pk_tag int PRIMARY KEY, label text);
INSERT INTO tb_csp_tag VALUES (1, '!');
CREATE FUNCTION csp_tag() RETURNS text LANGUAGE sql STABLE AS
    $$ SELECT label FROM public.tb_csp_tag WHERE pk_tag = 1 $$;
CREATE TABLE tb_csp_post (pk_csp_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          title text);
INSERT INTO tb_csp_post VALUES (1, DEFAULT, 'a');

-- The operators are found on this path; the names written below are unqualified.
SET search_path = public, regress_csp;
SELECT must((SELECT count(*) FROM pg_catalog.pg_class c
             WHERE c.oid = 'tb_csp_tag'::regclass) = 1, 'the operators are not reachable');
SELECT must((SELECT count(*) FROM csp_ran) > 0, 'the operators record nothing');
TRUNCATE csp_ran;

-- Each step runs on its own: its error is recorded, and the next one runs.
CREATE FUNCTION step(name text, sql text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_catalog.set_config('csp.step', name, false);
    EXECUTE sql;
EXCEPTION WHEN OTHERS THEN
    INSERT INTO public.csp_ran VALUES (current_user, 'error: ' || SQLERRM, name);
END $$;

-- Functions acting on one TVIEW: create (with options naming a table and a
-- function), replace, refresh, re-register, read.
SELECT step('create', $s$SELECT tviews.pg_tviews_create('tv_csp_post',
    $$SELECT pk_csp_post, id, jsonb_build_object('title', title || csp_tag()) AS data
      FROM tb_csp_post$$,
    '{"function_reads": {"csp_tag()": ["tb_csp_tag"]},
      "uncascaded_tables": {"tb_csp_tag": "full_refresh"}}')$s$);
SELECT must(to_regclass('public.tv_csp_post') IS NOT NULL,
            'an unqualified TVIEW is not created in the caller''s current schema');
SELECT step('replace', $s$SELECT tviews.pg_tviews_create_or_replace('tv_csp_post',
    $$SELECT pk_csp_post, id, jsonb_build_object('title', upper(title) || csp_tag()) AS data
      FROM tb_csp_post$$,
    '{"function_reads": {"csp_tag()": ["tb_csp_tag"]},
      "uncascaded_tables": {"tb_csp_tag": "full_refresh"}}')$s$);
SELECT step('refresh', $$SELECT tviews.pg_tviews_refresh('csp_post')$$);
SELECT step('reregister', $$SELECT tviews.pg_tviews_reregister('csp_post')$$);
SELECT step('show_cascade_path', $$SELECT tviews.pg_tviews_show_cascade_path('csp_post')$$);
SELECT step('mapping_query',
            $$SELECT tviews.pg_tviews_mapping_query('csp_post', 'tb_csp_post'::regclass)$$);
SELECT step('read_set_queries',
            $$SELECT tviews.pg_tviews_read_set_queries('csp_post', 'tb_csp_post'::regclass)$$);
SELECT step('entity_of', $$SELECT tviews.pg_tviews_entity_of('tv_csp_post')$$);
SELECT step('stats_reset', $$SELECT tviews.pg_tviews_stats_reset('csp_post')$$);

-- Functions acting on every TVIEW, and the status ones.
SELECT step('replication_status', $$SELECT tviews.pg_tviews_replication_status()$$);
SELECT step('health_check', $$SELECT tviews.pg_tviews_health_check()$$);
SELECT step('stats', $$SELECT count(*) FROM tviews.stats$$);
SELECT step('reregister_all', $$SELECT tviews.pg_tviews_reregister_all()$$);
SELECT step('rebuild_all', $$SELECT tviews.pg_tviews_rebuild_all()$$);
SELECT step('refresh_all', $$SELECT tviews.pg_tviews_refresh_all()$$);
SELECT step('ensure_propagation_indexes',
            $$SELECT tviews.pg_tviews_ensure_propagation_indexes(NULL, true)$$);
SELECT step('refresh_time_dependent', $$SELECT tviews.pg_tviews_refresh_time_dependent()$$);
SELECT step('queue_stats', $$SELECT tviews.pg_tviews_queue_stats()$$);
SELECT step('debug_queue', $$SELECT tviews.pg_tviews_debug_queue()$$);

-- Writes, and statements the hook follows.
SELECT step('write', $$UPDATE tb_csp_post SET title = 'b' WHERE pk_csp_post = 1$$);
SELECT step('write through a function', $$UPDATE tb_csp_tag SET label = '?' WHERE pk_tag = 1$$);
INSERT INTO csp_ran SELECT current_user, 'tv_csp_post does not follow the writes', 'write'
WHERE (SELECT data->>'title' FROM tv_csp_post WHERE pk_csp_post = 1) IS DISTINCT FROM 'B?';
SELECT step('rename column', $$ALTER TABLE tb_csp_post RENAME COLUMN title TO headline$$);
SELECT step('grant', $$GRANT SELECT ON tv_csp_post TO regress_csp$$);
SELECT step('create index', $$CREATE INDEX tv_csp_post_data ON tv_csp_post USING gin (data)$$);
SELECT step('drop index', $$DROP INDEX tv_csp_post_data$$);
SELECT step('create table as',
            $$CREATE TABLE tv_csp_copy AS SELECT pk_csp_post AS pk_csp_copy, id, data FROM tv_csp_post$$);
SELECT must(to_regclass('public.tv_csp_copy') IS NOT NULL,
            'CREATE TABLE tv_* AS does not create in the caller''s current schema');
SELECT step('drop table', $$DROP TABLE tv_csp_copy$$);
SELECT step('drop', $$SELECT tviews.pg_tviews_drop('csp_post')$$);

SELECT must(current_setting('search_path') = 'public, regress_csp',
            'the caller''s search_path is not restored');
SELECT must(NOT EXISTS (SELECT FROM csp_ran),
            'pg_tviews ran another role''s operator: '
            || (SELECT string_agg(DISTINCT step || ': ' || op || ' as ' || who, '; ') FROM csp_ran));

RESET search_path;
DROP TABLE tb_csp_post, tb_csp_tag, csp_ran;
DROP FUNCTION csp_tag();
DROP FUNCTION step(text, text);
DROP SCHEMA regress_csp CASCADE;
DROP ROLE regress_csp;

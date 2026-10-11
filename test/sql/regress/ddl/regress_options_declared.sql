-- A TVIEW is its definition and its options; no session setting changes it
-- (ADR 0220):
--   1. the defaults are fixed: LOGGED, fillfactor 85, no GIN index on data, the
--      `error` policy;
--   2. CREATE TABLE … AS, pg_tviews_create() and pg_tviews_create_or_replace()
--      create the same TVIEW;
--   3. the options passed are the whole declaration: an omitted option is at its
--      default;
--   4. tviews.registry.options holds every option, and passing it back changes
--      nothing;
--   5. the settings that described TVIEWs are gone; those deciding whether a
--      write succeeds are a superuser's.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_options_declared.sql
-- expect-output: options_declared: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'options_declared FAIL: %', what; END IF; END $$;
CREATE FUNCTION outcome(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'ok';
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ' ' || SQLERRM; END $$;
CREATE FUNCTION options_of(e text) RETURNS jsonb LANGUAGE sql AS $$
    SELECT options FROM tviews.registry WHERE entity = e $$;

CREATE TABLE tb_a (pk_a int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_b (pk_b int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_c (pk_c int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_a VALUES (1, DEFAULT, 'a');
INSERT INTO tb_b VALUES (1, DEFAULT, 'b');
INSERT INTO tb_c VALUES (1, DEFAULT, 'c');

-- 1 and 2. The defaults, the same on every path.
CREATE TABLE tv_a AS SELECT pk_a, id, jsonb_build_object('name', name) AS data FROM tb_a;
SELECT tviews.pg_tviews_create('tv_b', 'SELECT pk_b, id, jsonb_build_object(''name'', name) AS data FROM tb_b');
SELECT tviews.pg_tviews_create_or_replace('tv_c', 'SELECT pk_c, id, jsonb_build_object(''name'', name) AS data FROM tb_c');
SELECT must(options_of('a') = '{"logged": true, "fillfactor": 85, "data_gin_index": false,
                               "group_keys": null, "uncascaded_policy": "error",
                               "uncascaded_tables": {}, "function_reads": {},
                               "time_refresh": null, "typename": null}'::jsonb,
            'CREATE TABLE AS defaults: ' || options_of('a')::text);
SELECT must(options_of('b') = options_of('a'), 'pg_tviews_create: ' || options_of('b')::text);
SELECT must(options_of('c') = options_of('a'), 'pg_tviews_create_or_replace: ' || options_of('c')::text);
SELECT must((SELECT relpersistence FROM pg_class WHERE oid = 'tv_a'::regclass) = 'p', 'not LOGGED');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tview_valid), 'a LOGGED TVIEW has a validity row');
-- CREATE UNLOGGED TABLE … AS is the one storage option a CTAS carries.
CREATE TABLE tb_u (pk_u int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid());
CREATE UNLOGGED TABLE tv_u AS SELECT pk_u, id FROM tb_u;
SELECT must((options_of('u')->>'logged')::boolean = false, 'CREATE UNLOGGED TABLE AS');

-- 3. The options passed are the whole declaration.
SELECT must(tviews.pg_tviews_create_or_replace('tv_c',
                'SELECT pk_c, id, jsonb_build_object(''name'', name) AS data FROM tb_c',
                '{"logged": false, "fillfactor": 70, "typename": "Thing"}') = 'altered', 'declared');
SELECT must(options_of('c')->>'logged' = 'false' AND options_of('c')->>'fillfactor' = '70'
            AND options_of('c')->>'typename' = 'Thing', 'declared options: ' || options_of('c')::text);
SELECT must(tviews.pg_tviews_create_or_replace('tv_c',
                'SELECT pk_c, id, jsonb_build_object(''name'', name) AS data FROM tb_c') = 'altered',
            'omitted options go back to their defaults');
SELECT must(options_of('c') = options_of('a'), 'back to the defaults: ' || options_of('c')::text);
UPDATE tb_c SET name = 'c!';
SELECT assert_fresh('tv_c', 'pk_c', 'after the options changed in place');

-- 4. Every option published, and passed back unchanged: an aggregate TVIEW with a
--    per-table policy, a declared function read, the time and a type name.
CREATE TABLE tb_order (pk_order int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_a int NOT NULL REFERENCES tb_a, total numeric, placed date);
CREATE TABLE tb_rate (pk_rate int PRIMARY KEY, pct int NOT NULL);
INSERT INTO tb_order VALUES (1, DEFAULT, 1, 10, current_date), (2, DEFAULT, 1, 5, current_date - 40);
INSERT INTO tb_rate VALUES (1, 20);
CREATE FUNCTION current_rate() RETURNS int LANGUAGE sql STABLE AS 'SELECT max(pct) FROM tb_rate';
SELECT tviews.pg_tviews_create_or_replace('tv_spend', $$
    SELECT o.fk_a AS pk_spend, a.id,
           jsonb_build_object('total', sum(o.total),
                              'recent', count(*) FILTER (WHERE o.placed >= CURRENT_DATE - 30),
                              'rate', current_rate(),
                              'flags', (SELECT count(*) FROM tb_b)) AS data
    FROM tb_order o JOIN tb_a a ON a.pk_a = o.fk_a
    GROUP BY o.fk_a, a.id $$,
    '{"group_keys": {"tb_order": "fk_a", "tb_a": "pk_a"},
      "logged": false, "fillfactor": 90, "data_gin_index": true,
      "uncascaded_policy": "warn", "uncascaded_tables": {"public.tb_b": "full_refresh"},
      "function_reads": {"public.current_rate()": ["public.tb_rate"]},
      "time_refresh": "external", "typename": "Spend"}');
SELECT must(options_of('spend') = '{"group_keys": {"tb_order": "fk_a", "tb_a": "pk_a"},
      "logged": false, "fillfactor": 90, "data_gin_index": true,
      "uncascaded_policy": "warn", "uncascaded_tables": {"tb_b": "full_refresh"},
      "function_reads": {"public.current_rate()": ["tb_rate"]},
      "time_refresh": "external", "typename": "Spend"}'::jsonb,
            'every option published: ' || options_of('spend')::text);
SELECT must(tviews.pg_tviews_create_or_replace(format('%I.%I', schema, name), query, options) = 'unchanged',
            'round trip of ' || entity || ': ' || options::text)
FROM tviews.registry;
SELECT assert_fresh('tv_spend', 'pk_spend', 'the declared aggregate');

-- 5. The settings that described TVIEWs are gone; limits are a superuser's.
SELECT must(outcome(format('SET pg_tviews.%s = %L', s, v)) LIKE '%pg_tviews.' || s || '%',
            'pg_tviews.' || s || ' still exists')
FROM (VALUES ('unlogged_by_default', 'off'), ('fillfactor', '90'), ('data_gin_index', 'on'),
             ('uncascaded_policy', 'warn'), ('time_refresh', 'external'),
             ('union_duplicate_policy', 'first'), ('suspend_triggers', 'on'),
             ('log_level', 'debug')) AS r(s, v);
CREATE ROLE regress_options_user;
SET ROLE regress_options_user;
SELECT must(outcome(format('SET pg_tviews.%s = %L', s, v)) LIKE '42501 %',
            'a user may set pg_tviews.' || s)
FROM (VALUES ('max_propagation_depth', '5'), ('max_dependency_depth', '5'),
             ('max_queue_size', '5'), ('lock_escalation_threshold', '0'),
             ('audit_enabled', 'off'), ('direct_patch_enabled', 'off')) AS r(s, v);
SELECT must(outcome('SET pg_tviews.batch_size = 10') = 'ok', 'a user may not tune batch_size');
RESET ROLE;
DROP ROLE regress_options_user;
SELECT must(NOT EXISTS (SELECT 1 FROM pg_settings WHERE name IN
                ('pg_tviews.graph_cache_enabled', 'pg_tviews.table_cache_enabled',
                 'pg_tviews.direct_patch_enabled', 'pg_tviews.test_skip_ctas_intercept')),
            'diagnostic switches listed by SHOW ALL');

\echo 'options_declared: PASS'

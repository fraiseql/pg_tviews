-- One name for a TVIEW across the API (#211, ADR 0211): every function acting on
-- one TVIEW takes it as `tview`, spelled as its entity, tv_<entity>, or its
-- table qualified by its schema, quoted or not; a name that names no TVIEW fails
-- with 42704 and the same message everywhere; messages name the relation; the
-- functions ADR 0211 removes are gone.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_tview_name_forms.sql
-- expect-output: tview_name_forms: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'tview_name_forms FAIL: %', what; END IF; END $$;
CREATE FUNCTION outcome(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'ok';
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ' ' || SQLERRM; END $$;

CREATE SCHEMA app;
CREATE TABLE app.tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text, since date);
INSERT INTO app.tb_user VALUES (1, DEFAULT, 'ann', current_date);
SELECT tviews.pg_tviews_create('app.tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'new', since >= CURRENT_DATE) AS data
    FROM app.tb_user $$, '{"time_refresh": "external"}');

-- 1. Every per-TVIEW function takes every spelling.
SELECT must(outcome(format(call, form)) = 'ok', format(call, form) || ': ' || outcome(format(call, form)))
FROM unnest(ARRAY['user', 'tv_user', 'app.tv_user', '"app"."tv_user"', 'app."tv_user"']) AS form,
     unnest(ARRAY[
         'SELECT tviews.pg_tviews_refresh(%L)',
         'SELECT tviews.pg_tviews_reregister(%L)',
         'SELECT tviews.pg_tviews_refresh_time_dependent(%L)',
         'SELECT tviews.pg_tviews_ensure_propagation_indexes(%L, true)',
         'SELECT * FROM tviews.pg_tviews_show_cascade_path(%L)',
         'SELECT tviews.pg_tviews_mapping_query(%L, ''app.tb_user''::regclass)',
         'SELECT * FROM tviews.pg_tviews_read_set_queries(%L, ''app.tb_user''::regclass)',
         'SELECT * FROM tviews.pg_tviews_profile(%L)',
         $c$SELECT tviews.pg_tviews_create_or_replace(%L, (SELECT query FROM tviews.registry WHERE entity = 'user'), (SELECT options FROM tviews.registry WHERE entity = 'user'))$c$
     ]) AS call;

-- 2. A name that names no TVIEW: 42704, the same message everywhere. A schema
--    that is not the TVIEW's names no TVIEW.
SELECT must(outcome(format(call, form)) = '42704 TVIEW ' || form || ' does not exist',
            format(call, form) || ': ' || outcome(format(call, form)))
FROM unnest(ARRAY['nobody', 'tv_nobody', 'public.tv_user']) AS form,
     unnest(ARRAY[
         'SELECT tviews.pg_tviews_refresh(%L)',
         'SELECT tviews.pg_tviews_reregister(%L)',
         'SELECT tviews.pg_tviews_refresh_time_dependent(%L)',
         'SELECT tviews.pg_tviews_mapping_query(%L, ''app.tb_user''::regclass)',
         'SELECT * FROM tviews.pg_tviews_profile(%L)'
     ]) AS call;
SELECT must(outcome('SELECT tviews.pg_tviews_drop(''nobody'')') LIKE '42704 %TVIEW nobody does not exist',
            'drop of nothing: ' || outcome('SELECT tviews.pg_tviews_drop(''nobody'')'));

-- 3. One parameter name, tview, wherever a function takes a TVIEW.
SELECT must(string_agg(p.oid::regprocedure::text, ', ') IS NULL,
            'functions taking a TVIEW under another name: ' || string_agg(p.oid::regprocedure::text, ', '))
FROM pg_proc p
WHERE p.pronamespace = 'tviews'::regnamespace
  AND p.proname <> 'pg_tviews_handle_dropped'
  AND EXISTS (SELECT 1 FROM generate_subscripts(p.proargnames, 1) AS i
              WHERE coalesce(p.proargmodes[i], 'i') IN ('i', 'b')
                AND p.proargnames[i] IN ('tview_name', 'entity', 'entity_name', 'p_entity'));
SELECT must((SELECT string_agg(a, ',') FROM unnest(proargnames) a WHERE a IN ('entity_name'))
            IS NULL, 'show_cascade_path still returns entity_name')
FROM pg_proc WHERE proname = 'pg_tviews_show_cascade_path';

-- 4. Messages name the relation.
SELECT must(m = 'TVIEW app.tv_user dropped', 'drop message: ' || m)
FROM tviews.pg_tviews_drop('user') AS m;

-- 5. The functions ADR 0211 removes are gone.
SELECT must(NOT EXISTS (SELECT 1 FROM pg_proc WHERE pronamespace = 'tviews'::regnamespace
                        AND proname IN ('pg_tviews_create_aggregate', 'pg_tviews_refresh_all_entities',
                                        'pg_tviews_recover_after_crash', 'pg_tviews_set_logged',
                                        'pg_tviews_is_replica_readable', 'pg_tviews_performance_stats',
                                        'pg_tviews_set_typename')),
            'a removed function still exists');

\echo 'tview_name_forms: PASS'

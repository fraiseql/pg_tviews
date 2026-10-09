-- Regression test for issue #200: a refresh renders values under fixed settings,
-- not the writing session's. TimeZone, DateStyle, IntervalStyle,
-- extra_float_digits and bytea_output change how a timestamptz, a date, an
-- interval, a float or a bytea reads as text or JSON; every path that computes a
-- TVIEW's rows (create, a write, create_or_replace, pg_tviews_refresh, the time
-- refresh) pins them to TimeZone UTC, DateStyle 'ISO, YMD', IntervalStyle
-- postgres, extra_float_digits 1, bytea_output hex, and leaves the session's own
-- settings as they were.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_200_render_settings.sql
--
-- expect-output: issue #200 render settings: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#200 FAIL: %', what; END IF; END $$;
-- The TVIEW equals its definition evaluated under the pinned settings.
CREATE FUNCTION assert_pinned(tv regclass, key text, label text) RETURNS void LANGUAGE plpgsql
SET TimeZone = 'UTC' SET DateStyle = 'ISO, YMD' SET IntervalStyle = 'postgres'
SET extra_float_digits = 1 SET bytea_output = 'hex' AS $$
DECLARE diff text := fresh_diff(tv, key);
BEGIN
    IF diff IS NOT NULL THEN
        RAISE EXCEPTION '#200 FAIL: rendered in the writer''s settings after %: %', label, diff;
    END IF;
END $$;
CREATE FUNCTION settings() RETURNS text LANGUAGE sql AS $$
    SELECT concat_ws(' | ', current_setting('TimeZone'), current_setting('DateStyle'),
                     current_setting('IntervalStyle'), current_setting('extra_float_digits'),
                     current_setting('bytea_output'), current_setting('search_path')) $$;
-- Run `stmt` and check the session's settings are unchanged by it.
CREATE FUNCTION keeps_settings(stmt text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE before text := settings();
BEGIN
    EXECUTE stmt;
    PERFORM must(settings() = before, format('%s changed the session settings: %s -> %s', stmt, before, settings()));
END $$;

CREATE TABLE tb_event (pk_event bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       at timestamptz NOT NULL, day date, span interval, f float8, b bytea);
INSERT INTO tb_event VALUES
    (1, default, '2026-10-07 12:00+00', '2026-10-07', '1 day 2 hours', 0.1::float8 + 0.2::float8, '\x01ff'),
    (2, default, '2026-01-31 23:30+00', '2026-01-31', '-3 months 4 seconds', 1e-7, 'abc');

\set def 'SELECT pk_event, id, jsonb_build_object(''at'', at, ''at_text'', at::text, ''day'', day::text, ''span'', span::text, ''f'', f, ''f_text'', f::text, ''b'', b::text) AS data FROM public.tb_event'

-- The writer's settings, none of them the pinned value.
SET TIME ZONE 'Europe/Paris';
SET DateStyle = 'SQL, DMY';
SET IntervalStyle = 'sql_standard';
SET extra_float_digits = 0;
SET bytea_output = 'escape';

-- 1. Create (the initial fill).
SELECT keeps_settings(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_event', :'def'));
SELECT assert_pinned('tv_event', 'pk_event', 'create');

-- 2. A write from other settings (the flush).
SET TIME ZONE 'America/New_York';
SET DateStyle = 'German';
SET IntervalStyle = 'iso_8601';
SELECT keeps_settings($$UPDATE tb_event SET at = at + interval '1 hour' WHERE pk_event = 1$$);
SELECT keeps_settings($$INSERT INTO tb_event VALUES (3, default, '2026-03-29 01:30+00', '2026-03-29', '90 minutes', 2.5, '\x00')$$);
SELECT assert_pinned('tv_event', 'pk_event', 'a write');

-- 3. create_or_replace with a new definition (the rebuild).
SET TIME ZONE 'Asia/Kolkata';
SELECT keeps_settings(format('SELECT tviews.pg_tviews_create_or_replace(%L, %L)', 'tv_event',
    :'def' || ' WHERE pk_event > 0'));
SELECT assert_pinned('tv_event', 'pk_event', 'create_or_replace');

-- 4. pg_tviews_refresh (a full rebuild).
SET TIME ZONE 'Australia/Lord_Howe';
SELECT keeps_settings($$SELECT tviews.pg_tviews_refresh('event')$$);
SELECT assert_pinned('tv_event', 'pk_event', 'pg_tviews_refresh');

-- 5. The time refresh, called from +14 (the reporter's spike).
CREATE TABLE tb_alloc (pk_alloc bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       starts_at timestamptz NOT NULL);
INSERT INTO tb_alloc VALUES (1, default, '2026-10-07 08:00+00'), (2, default, '2999-01-01 00:00+00');
SELECT keeps_settings($$SELECT tviews.pg_tviews_create_or_replace('tv_alloc',
    'SELECT pk_alloc, id, jsonb_build_object(''starts_at'', starts_at, ''started'', starts_at <= now()) AS data FROM public.tb_alloc',
    '{"time_refresh": "external"}')$$);
SELECT assert_pinned('tv_alloc', 'pk_alloc', 'create (time-dependent)');
SET TIME ZONE 'Pacific/Kiritimati';
SELECT keeps_settings($$SELECT count(*) FROM tviews.pg_tviews_refresh_time_dependent()$$);
SELECT assert_pinned('tv_alloc', 'pk_alloc', 'pg_tviews_refresh_time_dependent()');
SELECT assert_pinned('tv_event', 'pk_event', 'pg_tviews_refresh_time_dependent() (other TVIEW)');

-- 6. A TVIEW read through another (a cascade), written from yet other settings.
CREATE TABLE tb_log (pk_log bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     fk_event bigint REFERENCES tb_event, at timestamptz);
INSERT INTO tb_log VALUES (1, default, 1, '2026-10-08 00:00+00');
SELECT keeps_settings($$SELECT tviews.pg_tviews_create('tv_log',
    'SELECT l.pk_log, l.id, l.fk_event, jsonb_build_object(''at'', l.at, ''event'', e.data) AS data
     FROM public.tb_log l JOIN public.tv_event e ON e.pk_event = l.fk_event')$$);
SET TIME ZONE 'America/St_Johns';
SET extra_float_digits = -2;
SELECT keeps_settings($$UPDATE tb_event SET f = 1.0 / 3 WHERE pk_event = 1$$);
SELECT assert_pinned('tv_event', 'pk_event', 'a cascade (tv_event)');
SELECT assert_pinned('tv_log', 'pk_log', 'a cascade (tv_log)');

SELECT 'issue #200 render settings: PASS' AS result;

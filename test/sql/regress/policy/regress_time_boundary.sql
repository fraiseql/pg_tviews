-- Regression test (P06, D5): the time refresh records the instant each TVIEW was
-- brought to, and tviews.registry shows it.
--   1. time_boundary is NULL for a TVIEW that reads no time, and for a time-dependent
--      one never refreshed at the boundary;
--   2. pg_tviews_refresh_time_dependent() sets it to its transaction's now(), the
--      instant its rows were evaluated at;
--   3. CALL pg_tviews_time_boundary() refreshes every time-dependent TVIEW the
--      caller owns, one transaction each, dependencies first, and records each;
--   4. dropping the TVIEW drops its boundary.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/policy/regress_time_boundary.sql
-- expect-output: time_boundary: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'time_boundary FAIL: %', what; END IF; END $$;
CREATE FUNCTION boundary(e text) RETURNS timestamptz LANGUAGE sql AS $$
    SELECT time_boundary FROM tviews.registry WHERE entity = e $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      ends_at timestamptz NOT NULL);
INSERT INTO tb_item VALUES (1, DEFAULT, now() + interval '1 day');
CREATE TABLE tv_plain AS SELECT pk_item AS pk_plain, id FROM tb_item;
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('current', now() < ends_at) AS data FROM tb_item $$,
    '{"time_refresh": "external"}');
SELECT tviews.pg_tviews_create('tv_summary', $$
    SELECT pk_item AS pk_summary, id, data->>'current' AS current, now()::date AS day
    FROM tv_item $$,
    '{"time_refresh": "external"}');

-- 1.
SELECT must(boundary('plain') IS NULL AND boundary('item') IS NULL, 'a boundary before any refresh');

-- 2.
BEGIN;
SELECT now() AS at \gset
SELECT * FROM tviews.pg_tviews_refresh_time_dependent('public.tv_item');
COMMIT;
SELECT must(boundary('item') = :'at'::timestamptz, 'item at ' || coalesce(boundary('item')::text, 'NULL'));
SELECT must(boundary('summary') IS NULL, 'summary recorded without its refresh');

-- 3.
SELECT pg_sleep(0.01);
CALL tviews.pg_tviews_time_boundary();
SELECT must(boundary('item') > :'at'::timestamptz AND boundary('summary') >= boundary('item'),
            'CALL: item ' || boundary('item') || ', summary ' || coalesce(boundary('summary')::text, 'NULL'));
-- One transaction each: the dependency's boundary is earlier than its reader's.
SELECT must(boundary('summary') > boundary('item'), 'one transaction for both');
SELECT must(boundary('plain') IS NULL, 'a boundary for a TVIEW that reads no time');

-- 4.
DROP TABLE tv_summary;
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tview_time_boundary WHERE entity = 'summary'),
            'the dropped TVIEW kept its boundary');

\echo 'time_boundary: PASS'

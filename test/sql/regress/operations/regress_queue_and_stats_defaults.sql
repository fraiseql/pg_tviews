-- Regression test: a bulk load passes the default queue limit, and
-- tviews.stats always says when a TVIEW's counters started.
--
-- pg_tviews.max_queue_size defaulted to 10,000 keys per transaction, so a
-- 20,000-row INSERT ... SELECT into a base table failed at defaults; it is
-- 1,000,000 now (a key is a few bytes). tviews.stats.stats_reset was NULL for a
-- TVIEW that had counted nothing yet; it is the server's start, or the reset
-- that zeroed it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_queue_and_stats_defaults.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'defaults FAIL: %', what; END IF; END $$;

CREATE TABLE tb_row (pk_row int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v int);
CREATE TABLE tb_idle (pk_idle int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v int);
SELECT tviews.pg_tviews_create('tv_row', $$
    SELECT pk_row, id, jsonb_build_object('v', v) AS data FROM tb_row $$);
SELECT tviews.pg_tviews_create('tv_idle', $$
    SELECT pk_idle, id, jsonb_build_object('v', v) AS data FROM tb_idle $$);

-- 1. The default limit, and a bulk load under it.
SELECT must(current_setting('pg_tviews.max_queue_size')::int = 1000000,
            'max_queue_size is ' || current_setting('pg_tviews.max_queue_size'));
INSERT INTO tb_row SELECT g, gen_random_uuid(), g FROM generate_series(1, 20000) g;
SELECT must((SELECT count(*) FROM tv_row) = 20000, 'tv_row holds the 20,000 rows');
-- A limit set low still holds.
SET pg_tviews.max_queue_size = 10;
DO $$ BEGIN
    INSERT INTO tb_row SELECT g, gen_random_uuid(), g FROM generate_series(20001, 20100) g;
    RAISE EXCEPTION 'defaults FAIL: 100 rows passed a limit of 10';
EXCEPTION WHEN program_limit_exceeded THEN NULL;
END $$;
RESET pg_tviews.max_queue_size;

-- 2. stats_reset: the server's start for a TVIEW that counted nothing.
SELECT must(stats_reset IS NOT NULL AND stats_reset = pg_postmaster_start_time(),
            'tv_idle stats_reset ' || coalesce(stats_reset::text, 'NULL'))
FROM tviews.stats WHERE entity = 'idle';
SELECT must(stats_reset IS NOT NULL, 'tv_row stats_reset') FROM tviews.stats WHERE entity = 'row';
-- After a reset of every TVIEW: the time of the reset, counted or not.
SELECT clock_timestamp() AS before_reset \gset
SELECT tviews.pg_tviews_stats_reset();
SELECT must(bool_and(stats_reset >= :'before_reset'::timestamptz) AND count(*) = 2,
            'after a reset: ' || string_agg(entity || '=' || coalesce(stats_reset::text, 'NULL'), ', '))
FROM tviews.stats;
-- One TVIEW reset alone.
SELECT clock_timestamp() AS before_one \gset
SELECT tviews.pg_tviews_stats_reset('idle');
SELECT must((SELECT stats_reset FROM tviews.stats WHERE entity = 'idle') >= :'before_one'::timestamptz
            AND (SELECT stats_reset FROM tviews.stats WHERE entity = 'row') < :'before_one'::timestamptz,
            'reset of one TVIEW');

SELECT 'queue and stats defaults: PASS' AS result;
-- expect-output: queue and stats defaults: PASS

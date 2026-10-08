-- Regression test for issue #193 (time): a definition that reads the time
-- (CURRENT_DATE, now() and the like, directly or in a view, a subquery or a CTE)
-- has rows that change with no write. It is refused under the error and
-- full_refresh policies unless it declares "time_refresh": "external", and warned
-- about under warn. A time-dependent TVIEW is reported by tviews.registry, and
-- tviews.pg_tviews_refresh_time_dependent() brings it up to date, for pg_cron or
-- the application to call at the boundary.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/policy/regress_time_dependent.sql
--
-- expect-output: issue #193 time: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#193 FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'created';
EXCEPTION WHEN OTHERS THEN RETURN SQLERRM; END $$;

SET pg_tviews.uncascaded_policy = 'error';
CREATE TABLE tb_contract (pk_contract bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text, end_date date, ends_at timestamptz);
INSERT INTO tb_contract VALUES (1, default, 'c1', current_date, now()), (2, default, 'c2', current_date + 1, now());

\set def 'SELECT pk_contract, id, name, (end_date >= CURRENT_DATE) AS is_current FROM tb_contract'

-- 1. The issue: refused under error, the construct named.
SELECT must(outcome LIKE '%reads the time (CURRENT_DATE)%time_refresh%', 'the issue: ' || outcome)
FROM (SELECT error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_contract', :'def')) AS outcome) o;

-- 2. Every construct, refused and named; under full_refresh too.
SELECT must(outcome LIKE '%reads the time (' || named || ')%', expr || ': ' || outcome)
FROM (VALUES ('CURRENT_DATE', 'CURRENT_DATE'), ('CURRENT_TIMESTAMP', 'CURRENT_TIMESTAMP'),
             ('CURRENT_TIMESTAMP(0)', 'CURRENT_TIMESTAMP'), ('CURRENT_TIME', 'CURRENT_TIME'),
             ('LOCALTIMESTAMP', 'LOCALTIMESTAMP'), ('LOCALTIME(2)', 'LOCALTIME'),
             ('now()', 'now()'), ('clock_timestamp()', 'clock_timestamp()'),
             ('statement_timestamp()', 'statement_timestamp()'),
             ('transaction_timestamp()', 'transaction_timestamp()'), ('timeofday()', 'timeofday()'),
             ('age(ends_at)', 'age()'), ('age(end_date::timestamp)', 'age()')) AS c(expr, named),
     LATERAL (SELECT error_of(format($$SELECT tviews.pg_tviews_create_or_replace('tv_contract',
         'SELECT pk_contract, id, (%s)::text AS t FROM tb_contract', '{"uncascaded_policy": "full_refresh"}')$$,
         expr)) AS outcome) o;
-- Not the time: age() of two values, a date literal, CURRENT_USER.
SELECT tviews.pg_tviews_create('tv_contract', $$SELECT pk_contract, id, age(ends_at, ends_at) AS a,
    DATE '2026-01-01' AS d, CURRENT_USER AS u FROM tb_contract$$);
SELECT must(NOT time_dependent AND time_refresh IS NULL, 'a TVIEW reading no time')
FROM tviews.registry WHERE entity = 'contract';
SELECT tviews.pg_tviews_drop('tv_contract');

-- 3. Through a view, in a subquery, in a CTE, and in a WHERE.
CREATE VIEW v_current AS SELECT pk_contract, name FROM tb_contract WHERE end_date >= CURRENT_DATE;
SELECT must(outcome LIKE '%reads the time (' || named || ')%', how || ': ' || outcome)
FROM (VALUES ('view', 'CURRENT_DATE', $$SELECT c.pk_contract, c.id, v.name FROM tb_contract c LEFT JOIN v_current v ON v.pk_contract = c.pk_contract$$),
             ('subquery', 'now()', $$SELECT c.pk_contract, c.id, (SELECT now() - c.ends_at) AS since FROM tb_contract c$$),
             ('cte', 'LOCALTIMESTAMP', $$WITH t AS (SELECT LOCALTIMESTAMP AS at) SELECT c.pk_contract, c.id, t.at FROM tb_contract c, t$$),
             ('where', 'CURRENT_DATE', $$SELECT c.pk_contract, c.id, c.name FROM tb_contract c WHERE c.end_date >= CURRENT_DATE$$))
     AS c(how, named, def),
     LATERAL (SELECT error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_contract', def)) AS outcome) o;

-- 4. Declared: created, reported, refreshed on demand.
SELECT must(tviews.pg_tviews_create_or_replace('tv_contract', :'def', '{"time_refresh": "external"}') = 'created',
            'declared');
SELECT must(time_dependent AND time_refresh = 'external', 'registry: ' || time_dependent || ' ' || coalesce(time_refresh, 'NULL'))
FROM tviews.registry WHERE entity = 'contract';
-- Rows that changed with no write the TVIEW saw (as at midnight): suspended triggers.
SET pg_tviews.suspend_triggers = on;
UPDATE tb_contract SET end_date = current_date - 1 WHERE pk_contract = 1;
RESET pg_tviews.suspend_triggers;
SELECT must(fresh_diff('tv_contract', 'pk_contract') IS NOT NULL, 'the TVIEW was not stale');
SELECT must(array_agg(r) = ARRAY['public.tv_contract'], 'refreshed: ' || array_agg(r)::text)
FROM tviews.pg_tviews_refresh_time_dependent() r;
SELECT assert_fresh('tv_contract', 'pk_contract', 'pg_tviews_refresh_time_dependent()');
SELECT must((SELECT NOT is_current FROM tv_contract WHERE pk_contract = 1), 'is_current did not flip');
-- One TVIEW by name; one that reads no time is refused.
SELECT tviews.pg_tviews_refresh_time_dependent('tv_contract');
CREATE TABLE tb_party (pk_party bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
SELECT tviews.pg_tviews_create('tv_party', 'SELECT pk_party, id, name FROM tb_party');
SELECT must(error_of('SELECT tviews.pg_tviews_refresh_time_dependent(''tv_party'')') LIKE '%tv_party%does not read the time%',
            'a TVIEW reading no time');
-- The same again: unchanged; re-registration keeps it.
SELECT must(tviews.pg_tviews_create_or_replace('tv_contract', :'def', '{"time_refresh": "external"}') = 'unchanged',
            'the same declaration');
SELECT tviews.pg_tviews_reregister('contract');
SELECT must(time_dependent AND time_refresh = 'external', 'after reregister')
FROM tviews.registry WHERE entity = 'contract';
-- Writes still refresh it as usual.
UPDATE tb_contract SET name = 'C1' WHERE pk_contract = 1;
SELECT assert_fresh('tv_contract', 'pk_contract', 'a write');

-- 5. Fail loud: declared, but the definition reads no time; an unknown value.
SELECT must(error_of($$SELECT tviews.pg_tviews_create_or_replace('tv_party', 'SELECT pk_party, id, name FROM tb_party',
                       '{"time_refresh": "external"}')$$) LIKE '%time_refresh%reads no time%', 'declared, no time');
SELECT must(error_of($$SELECT tviews.pg_tviews_create_or_replace('tv_party', 'SELECT pk_party, id, name FROM tb_party',
                       '{"time_refresh": "daily"}')$$) LIKE '%time_refresh%', 'an unknown value');

-- 6. The setting, for CREATE TABLE … AS and pg_tviews_create(): applies to a
--    TVIEW reading the time, and is ignored by one that reads none.
SELECT tviews.pg_tviews_drop('tv_contract');
DROP TABLE tb_party CASCADE;
CREATE TABLE tb_party (pk_party bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
SET pg_tviews.time_refresh = 'external';
SELECT tviews.pg_tviews_create('tv_contract', :'def');
SELECT tviews.pg_tviews_create('tv_party', 'SELECT pk_party, id, name FROM tb_party');
RESET pg_tviews.time_refresh;
SELECT must(string_agg(entity || '=' || time_dependent || '/' || coalesce(time_refresh, 'NULL'), ',' ORDER BY entity)
            = 'contract=true/external,party=false/NULL', 'the setting: ' || string_agg(entity || '=' || time_dependent || '/' || coalesce(time_refresh, 'NULL'), ','))
FROM tviews.registry;
SELECT tviews.pg_tviews_drop('tv_contract');

-- 7. Under warn: created, warned, reported as time-dependent and refreshable.
SET pg_tviews.uncascaded_policy = 'warn';
SELECT tviews.pg_tviews_create('tv_contract', :'def');
SELECT must(time_dependent AND time_refresh IS NULL, 'warn: registry')
FROM tviews.registry WHERE entity = 'contract';
SELECT must(array_agg(r) = ARRAY['public.tv_contract'], 'warn: refreshed ' || array_agg(r)::text)
FROM tviews.pg_tviews_refresh_time_dependent() r;

-- 8. A TVIEW registered before time was detected (an upgrade from beta.25, under
--    error): pg_tviews_reregister_all() lists the refusal, and the TVIEW keeps its
--    registration and keeps refreshing on writes.
CREATE TABLE tb_legacy (pk_legacy bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        end_date date);
INSERT INTO tb_legacy VALUES (1, default, current_date);
SELECT tviews.pg_tviews_create('tv_legacy', 'SELECT pk_legacy, id, end_date >= CURRENT_DATE AS is_current FROM tb_legacy');
UPDATE tviews.pg_tview_meta SET uncascaded_policy = 'error', time_dependent = false WHERE entity = 'legacy';
SELECT must(status LIKE '%reads the time (CURRENT_DATE)%', 'reregister_all: ' || status)
FROM tviews.pg_tviews_reregister_all() WHERE entity = 'legacy';
SELECT must(uncascaded_policy = 'error' AND NOT time_dependent, 'the old registration is kept')
FROM tviews.registry WHERE entity = 'legacy';
UPDATE tb_legacy SET end_date = current_date - 1;
SELECT assert_fresh('tv_legacy', 'pk_legacy', 'a write after the refused re-registration');

\echo issue #193 time: PASS

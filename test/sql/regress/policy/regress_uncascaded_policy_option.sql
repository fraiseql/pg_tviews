-- A TVIEW declares what a write to a table no cascade reaches does, with the
-- `uncascaded_policy` option of pg_tviews_create_or_replace(): stored with the
-- TVIEW, changed in place ("altered", no rebuild), and `error` when omitted
-- (ADR 0220).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/policy/regress_uncascaded_policy_option.sql
--
-- expect-output: uncascaded_policy option: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'uncascaded_policy option FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLERRM; END $$;
CREATE FUNCTION policy_of(e text) RETURNS text LANGUAGE sql AS $$
    SELECT options->>'uncascaded_policy' FROM tviews.registry WHERE entity = e $$;

CREATE TABLE tb_flag (pk_flag int PRIMARY KEY, on_off boolean NOT NULL);
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_flag VALUES (1, true);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'a'), (2, 'b');

-- tb_flag is read in an uncorrelated subquery: no cascade reaches it.
\set def 'SELECT pk_item, id, jsonb_build_object(''name'', name, ''flags'', (SELECT count(*) FROM tb_flag WHERE on_off)) AS data FROM tb_item'

SELECT must(tviews.pg_tviews_create_or_replace('tv_item', :'def', '{"uncascaded_policy": "full_refresh"}') = 'created',
            'created');
SELECT must(policy_of('item') = 'full_refresh', 'stored ' || policy_of('item'));
INSERT INTO tb_flag VALUES (2, true);
SELECT assert_fresh('tv_item', 'pk_item', 'a write to tb_flag under full_refresh');

-- The same declaration: unchanged. No option is the default, `error`, which
-- refuses this TVIEW; nothing changes.
SELECT must(tviews.pg_tviews_create_or_replace('tv_item', :'def', '{"uncascaded_policy": "full_refresh"}') = 'unchanged',
            'the same option');
SELECT must(error_of(format('SELECT tviews.pg_tviews_create_or_replace(%L, %L)', 'tv_item', :'def'))
            LIKE '%would not refresh%', 'no option is error');
SELECT must(policy_of('item') = 'full_refresh', 'a refused change kept ' || policy_of('item'));

-- Another policy: altered in place, rows and table kept.
CREATE TEMP TABLE before AS SELECT 'tv_item'::regclass::oid AS tv, xmin::text AS x FROM tv_item WHERE pk_item = 1;
SELECT must(tviews.pg_tviews_create_or_replace('tv_item', :'def', '{"uncascaded_policy": "warn"}') = 'altered',
            'another policy');
SELECT must(policy_of('item') = 'warn', 'altered to ' || policy_of('item'));
SELECT must((SELECT tv FROM before) = 'tv_item'::regclass::oid
            AND (SELECT x FROM before) = (SELECT xmin::text FROM tv_item WHERE pk_item = 1),
            'the table was rebuilt or its rows rewritten');

-- `error` refuses a TVIEW with such a table, and nothing changes.
SELECT must(error_of(format('SELECT tviews.pg_tviews_create_or_replace(%L, %L, %L)',
                            'tv_item', :'def', '{"uncascaded_policy": "error"}'))
            LIKE '%would not refresh%', 'error refused');
SELECT must(policy_of('item') = 'warn', 'a refused change kept ' || policy_of('item'));

-- A new definition carries the declared policy (replaced in place, rebuilt).
SELECT must(tviews.pg_tviews_create_or_replace('tv_item',
                replace(:'def', '''name'', name', '''name'', upper(name)'),
                '{"uncascaded_policy": "full_refresh"}') = 'replaced', 'replaced');
SELECT must(policy_of('item') = 'full_refresh', 'replaced with ' || policy_of('item'));
SELECT must(tviews.pg_tviews_create_or_replace('tv_item',
                replace(:'def', 'id, jsonb', 'id, name, jsonb'),
                '{"uncascaded_policy": "warn"}') = 'rebuilt', 'rebuilt');
SELECT must(policy_of('item') = 'warn', 'rebuilt with ' || policy_of('item'));

-- A value that is not a policy.
SELECT must(error_of($$SELECT tviews.pg_tviews_create_or_replace('tv_item', 'SELECT 1', '{"uncascaded_policy": "ignore"}')$$)
            LIKE '%uncascaded_policy%must be "error", "full_refresh" or "warn"%', 'an unknown value');

\echo 'uncascaded_policy option: PASS'

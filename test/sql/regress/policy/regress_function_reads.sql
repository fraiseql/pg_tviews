-- Regression test for issue #193 (function reads): a call to a non-immutable
-- function outside pg_catalog may read tables pg_tviews cannot see. Undeclared, it
-- is refused under the error and full_refresh policies (nothing would refresh the
-- TVIEW) and warned under warn. Declared in the function_reads option, the tables
-- it reads are read by the TVIEW, no cascade reaches them, and their policy says
-- what a write to them does.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/policy/regress_function_reads.sql
--
-- expect-output: issue #193 function reads: PASS

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

CREATE TABLE tb_setting (code text PRIMARY KEY, value text);
INSERT INTO tb_setting VALUES ('label_suffix', ' (a)');
CREATE FUNCTION label_suffix() RETURNS text STABLE LANGUAGE sql
  AS $$ SELECT value FROM public.tb_setting WHERE code = 'label_suffix' $$;
CREATE FUNCTION setting(c text) RETURNS text STABLE LANGUAGE sql
  AS $$ SELECT value FROM public.tb_setting WHERE code = c $$;
CREATE FUNCTION shout(t text) RETURNS text IMMUTABLE LANGUAGE sql AS $$ SELECT upper(t) $$;
CREATE FUNCTION tag() RETURNS text STABLE LANGUAGE sql AS $$ SELECT current_setting('application_name') $$;
CREATE TABLE tb_contract (pk_contract bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
INSERT INTO tb_contract VALUES (1, default, 'c1'), (2, default, 'c2');

\set def 'SELECT pk_contract, id, shout(name) || label_suffix() AS label FROM tb_contract'

-- 1. Undeclared: refused under error and under full_refresh, naming the function.
SELECT must(outcome LIKE '%public.label_suffix()%not immutable%function_reads%', 'error: ' || outcome)
FROM (SELECT error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_contract', :'def')) AS outcome) o;
SELECT must(outcome LIKE '%public.label_suffix()%function_reads%', 'full_refresh: ' || outcome)
FROM (SELECT error_of(format($$SELECT tviews.pg_tviews_create_or_replace(%L, %L,
        '{"uncascaded_policy": "full_refresh"}')$$, 'tv_contract', :'def')) AS outcome) o;

-- 2. Undeclared under warn: created, with a warning.
SELECT tviews.pg_tviews_create('tv_contract', :'def', '{"uncascaded_policy": "warn"}');
SELECT tviews.pg_tviews_drop('tv_contract');

-- 3. Declared, its table under no policy of its own: refused like any table no
--    cascade reaches, with the function as the reason.
SELECT must(outcome LIKE 'writes to public.tb_setting would not refresh public.tv_contract (read inside public.label_suffix())%',
            'declared: ' || outcome)
FROM (SELECT error_of(format($$SELECT tviews.pg_tviews_create_or_replace(%L, %L,
        '{"function_reads": {"public.label_suffix()": ["public.tb_setting"]}}')$$, 'tv_contract', :'def')) AS outcome) o;

-- 4. Declared, its table refreshed in full: the label follows tb_setting.
SELECT must(tviews.pg_tviews_create_or_replace('tv_contract', :'def', '{
  "function_reads": {"label_suffix()": ["tb_setting"]},
  "uncascaded_tables": {"tb_setting": "full_refresh"}}') = 'created', 'declared and full_refresh');
SELECT must(options->'function_reads' = '{"public.label_suffix()": ["tb_setting"]}'
            AND 'tb_setting'::regclass = ANY (base_tables) AND 'tb_setting'::regclass = ANY (uncascaded_tables)
            AND cascade_kinds ->> 'tb_setting' = 'all_keys',
            'registry: ' || (options->'function_reads')::text || ' ' || base_tables::text || ' ' || cascade_kinds::text)
FROM tviews.registry WHERE entity = 'contract';
UPDATE tb_setting SET value = ' (b)';
SELECT assert_fresh('tv_contract', 'pk_contract', 'a write to the table the function reads');
SELECT must((SELECT label FROM tv_contract WHERE pk_contract = 1) = 'C1 (b)', 'the label');
SELECT must((SELECT status FROM tviews.pg_tviews_health_check() WHERE component = 'triggers') = 'OK',
            'health: ' || (SELECT message FROM tviews.pg_tviews_health_check() WHERE component = 'triggers'));
-- The same again: unchanged; re-registration keeps it.
SELECT must(tviews.pg_tviews_create_or_replace('tv_contract', :'def', '{
  "function_reads": {"public.label_suffix()": ["public.tb_setting"]},
  "uncascaded_tables": {"tb_setting": "full_refresh"}}') = 'unchanged', 'the same declaration');
SELECT tviews.pg_tviews_reregister('contract');
UPDATE tb_setting SET value = ' (c)';
SELECT assert_fresh('tv_contract', 'pk_contract', 'after re-registration');

-- 5. A function that reads no table is declared with [].
SELECT must(tviews.pg_tviews_create_or_replace('tv_contract',
    'SELECT pk_contract, id, name || tag() || label_suffix() || setting(''x'') AS label FROM tb_contract', '{
  "function_reads": {"public.label_suffix()": ["public.tb_setting"], "public.tag()": [],
                     "setting(text)": ["public.tb_setting"]},
  "uncascaded_tables": {"tb_setting": "full_refresh"}}') = 'replaced', 'a function reading no table');
SELECT must(options->'function_reads' = '{"public.tag()": [], "public.label_suffix()": ["tb_setting"],
                               "public.setting(text)": ["tb_setting"]}',
            'registry: ' || (options->'function_reads')::text)
FROM tviews.registry WHERE entity = 'contract';

-- 6. Fail loud: a declared function the definition does not call, or that does
--    not exist, and a declared table that does not exist.
SELECT must(outcome LIKE '%public.tag()%not call%', 'uncalled: ' || outcome)
FROM (SELECT error_of(format($$SELECT tviews.pg_tviews_create_or_replace('tv_contract', %L, '{
  "function_reads": {"public.label_suffix()": ["public.tb_setting"], "public.tag()": []},
  "uncascaded_tables": {"tb_setting": "full_refresh"}}')$$, :'def')) AS outcome) o;
SELECT must(outcome LIKE '%nowhere()%', 'missing function: ' || outcome)
FROM (SELECT error_of(format($$SELECT tviews.pg_tviews_create_or_replace('tv_contract', %L, '{
  "function_reads": {"public.nowhere()": []}}')$$, :'def')) AS outcome) o;
SELECT must(outcome LIKE '%tb_nowhere%', 'missing table: ' || outcome)
FROM (SELECT error_of(format($$SELECT tviews.pg_tviews_create_or_replace('tv_contract', %L, '{
  "function_reads": {"public.label_suffix()": ["public.tb_nowhere"]}}')$$, :'def')) AS outcome) o;
SELECT must(outcome LIKE '%function_reads%', 'not a list: ' || outcome)
FROM (SELECT error_of(format($$SELECT tviews.pg_tviews_create_or_replace('tv_contract', %L, '{
  "function_reads": {"public.label_suffix()": "public.tb_setting"}}')$$, :'def')) AS outcome) o;

-- 7. A call inside a view the definition reads is found too; immutable and
--    pg_catalog functions need nothing.
CREATE VIEW v_contract AS SELECT pk_contract, id, name || label_suffix() AS label FROM tb_contract;
CREATE TABLE tb_party (pk_party bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_party VALUES (1, default, 'p');
SELECT must(outcome LIKE '%public.label_suffix()%', 'through a view: ' || outcome)
FROM (SELECT error_of($$SELECT tviews.pg_tviews_create('tv_party', 'SELECT p.pk_party, p.id, p.name,
        (SELECT max(label) FROM v_contract) AS label FROM tb_party p')$$) AS outcome) o;
SELECT tviews.pg_tviews_create('tv_party', $$SELECT pk_party, id, shout(name) AS name,
    current_setting('application_name') AS app FROM tb_party$$);

\echo issue #193 function reads: PASS

-- A TVIEW that reads a table no cascade reaches, and declares no
-- uncascaded_policy, is refused at create (the default is `error`): an agent fixes
-- an ERROR in its loop, it does not read WARNINGs. The refusal names each table
-- with its reason and says exactly what to write.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_uncascaded_policy_default.sql
--
-- expect-output: uncascaded_policy default: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'uncascaded_policy default FAIL: %', what; END IF; END $$;
-- The message and the hint of the error `stmt` raises.
CREATE FUNCTION refusal(stmt text, OUT message text, OUT hint text) LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE stmt;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS message = MESSAGE_TEXT, hint = PG_EXCEPTION_HINT;
END $$;

CREATE TABLE tb_flag (pk_flag int PRIMARY KEY, on_off boolean NOT NULL);
CREATE TABLE tb_rate (pk_rate int PRIMARY KEY, pct int NOT NULL);
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_flag VALUES (1, true);
INSERT INTO tb_rate VALUES (1, 5);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'a');

SELECT must(current_setting('pg_tviews.uncascaded_policy') = 'error', 'the default is '
            || current_setting('pg_tviews.uncascaded_policy'));

-- pg_tviews_create_or_replace() without the option.
SELECT * FROM refusal($q$SELECT tviews.pg_tviews_create_or_replace('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('name', name,
           'flags', (SELECT count(*) FROM tb_flag), 'rate', (SELECT max(pct) FROM tb_rate)) AS data
    FROM tb_item $$)$q$) \gset r_
SELECT must(:'r_message' LIKE 'writes to public.tb_flag, public.tb_rate would not refresh public.tv_item (%',
            'the message names the tables: ' || :'r_message');
SELECT must(:'r_message' LIKE '%public.tb_flag: read in a subquery%public.tb_rate: read in a subquery%',
            'the message gives each table''s reason: ' || :'r_message');
SELECT must(:'r_hint' LIKE '%options => ''{"uncascaded_tables": {"public.tb_flag": "full_refresh", "public.tb_rate": "full_refresh"}}''%',
            'the hint gives the per-table option: ' || :'r_hint');
SELECT must(:'r_hint' LIKE '%''{"uncascaded_policy": "full_refresh"}''%',
            'the hint gives the option: ' || :'r_hint');
SELECT must(:'r_hint' LIKE '%SET pg_tviews.uncascaded_policy = ''full_refresh''%',
            'the hint gives the setting: ' || :'r_hint');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.registry) AND to_regclass('tv_item') IS NULL,
            'a refused create left a TVIEW');

-- CREATE TABLE … AS (what confiture runs): the same refusal.
SELECT * FROM refusal($q$CREATE TABLE tv_item AS
    SELECT pk_item, id, jsonb_build_object('flags', (SELECT count(*) FROM tb_flag)) AS data FROM tb_item$q$) \gset c_
SELECT must(:'c_message' LIKE 'writes to public.tb_flag would not refresh public.tv_item (read in a subquery%',
            'CREATE TABLE AS: ' || :'c_message');
SELECT must(:'c_hint' LIKE '%SET pg_tviews.uncascaded_policy%', 'CREATE TABLE AS hint: ' || :'c_hint');

-- What the hint says to write works.
SELECT tviews.pg_tviews_create_or_replace('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('flags', (SELECT count(*) FROM tb_flag)) AS data FROM tb_item $$,
    '{"uncascaded_policy": "full_refresh"}');
SELECT must((SELECT uncascaded_policy FROM tviews.registry WHERE entity = 'item') = 'full_refresh',
            'the declared policy');
-- A TVIEW every read of which is traced needs no declaration.
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), body text);
CREATE TABLE tv_note AS SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM tb_note;
SELECT must((SELECT uncascaded_policy FROM tviews.registry WHERE entity = 'note') = 'error',
            'a fully traced TVIEW stores the default');

\echo 'uncascaded_policy default: PASS'

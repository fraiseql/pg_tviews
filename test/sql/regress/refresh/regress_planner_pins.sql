-- A writer's session or role does not steer how the owner's refresh is planned:
-- every frame pins the planner settings to the server's values. A value the
-- configuration file, the command line or ALTER DATABASE gave stays in force (a
-- DBA's tuning); a session's SET and a role's ALTER ROLE ... SET do not reach the
-- refresh. A function of the view reports the settings each evaluation runs
-- under; the session's own settings are untouched.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_planner_pins.sql
-- expect-output: planner_pins: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_pp');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'planner_pins FAIL: %', what; END IF; END $$;

-- The planner settings each evaluation runs under.
CREATE FUNCTION planner_seen(v text) RETURNS jsonb LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    RETURN jsonb_build_object('v', v,
        'enable_nestloop', pg_catalog.current_setting('enable_nestloop'),
        'enable_hashjoin', pg_catalog.current_setting('enable_hashjoin'),
        'work_mem', pg_catalog.current_setting('work_mem'),
        'random_page_cost', pg_catalog.current_setting('random_page_cost'),
        'join_collapse_limit', pg_catalog.current_setting('join_collapse_limit'));
END $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text NOT NULL);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'a');
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, planner_seen(i.name) AS data FROM tb_item i $$);
GRANT SELECT, UPDATE ON tb_item TO regress_pp_writer;
GRANT SELECT ON tv_item TO regress_pp_writer;

SELECT current_setting('work_mem') AS server_work_mem,
       current_setting('random_page_cost') AS server_random_page_cost,
       current_setting('join_collapse_limit') AS server_join_collapse_limit \gset

-- 1. A session's planner settings do not reach the refresh.
SET enable_nestloop = off;
SET enable_hashjoin = off;
SET work_mem = '64kB';
SET random_page_cost = 1000;
SET join_collapse_limit = 1;
UPDATE tb_item SET name = 'b' WHERE pk_item = 1;
SELECT must(data->>'enable_nestloop' = 'on' AND data->>'enable_hashjoin' = 'on',
            'a session''s enable_* reached the refresh: ' || data::text) FROM tv_item;
SELECT must(data->>'work_mem' = :'server_work_mem'
            AND data->>'random_page_cost' = :'server_random_page_cost'
            AND data->>'join_collapse_limit' = :'server_join_collapse_limit',
            'a session''s planner costs reached the refresh: ' || data::text) FROM tv_item;
SELECT must(current_setting('enable_nestloop') = 'off' AND current_setting('work_mem') = '64kB',
            'the session''s own settings changed');
-- A full refresh too.
SELECT tviews.pg_tviews_refresh('item');
SELECT must(data->>'enable_nestloop' = 'on', 'a full refresh used the session''s setting: ' || data::text)
FROM tv_item;
RESET ALL;

-- 2. ALTER DATABASE is the server's tuning: it stays in force. A role's setting
--    is the writer's own: it does not reach the refresh, and the database's
--    value applies under it.
SELECT format('ALTER DATABASE %I SET random_page_cost = 1.1', current_database()) \gexec
ALTER ROLE regress_pp_writer SET random_page_cost = 1000;
ALTER ROLE regress_pp_writer SET enable_nestloop = off;
\c - regress_pp_writer
SET client_min_messages TO WARNING;
SELECT must(current_setting('random_page_cost') = '1000', 'the writer''s role setting is not in force');
UPDATE tb_item SET name = 'c' WHERE pk_item = 1;
SELECT must(data->>'random_page_cost' = '1.1' AND data->>'enable_nestloop' = 'on',
            'a role''s setting reached the refresh, or the database''s did not: ' || data::text)
FROM tv_item;
\c - postgres
SET client_min_messages TO WARNING;
UPDATE tb_item SET name = 'd' WHERE pk_item = 1;
SELECT must(data->>'random_page_cost' = '1.1', 'the database''s setting did not reach the refresh: '
            || data::text) FROM tv_item;
SELECT format('ALTER DATABASE %I RESET random_page_cost', current_database()) \gexec

SELECT 'planner_pins: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_item;
SELECT regress_drop_roles('regress_pp');

-- Issue #290: DROP DATABASE frees the tviews.stats slots of its TVIEWs.
--
-- The statistics live in shared memory, a fixed table of slots keyed by database
-- and TVIEW. DROP DATABASE drops no table one by one, so a cluster that creates
-- and drops databases (test runs) filled the table for good. The ProcessUtility
-- hook frees the database's slots once the drop succeeds. Test builds list the
-- slots each database holds in pg_tviews_show_queue().
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_drop_database_stats.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE EXTENSION IF NOT EXISTS dblink;

SELECT tviews.pg_tviews_show_queue() ? 'stats_slots_by_database' AS hooks \gset
\if :hooks
\else
\echo 'SKIP: stats_slots_by_database needs a test-hooks build'
\quit
\endif

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#290 FAIL: %', what; END IF; END $$;
CREATE FUNCTION slots(db oid) RETURNS int LANGUAGE sql AS $$
    SELECT coalesce((tviews.pg_tviews_show_queue()->'stats_slots_by_database'->>db::text)::int, 0) $$;

DROP DATABASE IF EXISTS regress_290_other;
CREATE DATABASE regress_290_other;
SELECT oid AS other FROM pg_database WHERE datname = 'regress_290_other' \gset
SELECT format('host=localhost dbname=regress_290_other user=%s port=%s', current_user,
              current_setting('port')) AS conninfo \gset

-- Two TVIEWs written in the other database: two slots.
SELECT dblink_exec(:'conninfo', $$
    CREATE EXTENSION jsonb_delta;
    CREATE EXTENSION pg_tviews;
    CREATE TABLE tb_a (pk_a int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v text);
    CREATE TABLE tb_b (pk_b int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v text);
    SELECT tviews.pg_tviews_create('tv_a', 'SELECT pk_a, id, jsonb_build_object(''v'', v) AS data FROM tb_a');
    SELECT tviews.pg_tviews_create('tv_b', 'SELECT pk_b, id, jsonb_build_object(''v'', v) AS data FROM tb_b');
    INSERT INTO tb_a (pk_a, v) VALUES (1, 'x');
    INSERT INTO tb_b (pk_b, v) VALUES (1, 'y');
$$) \g /dev/null
SELECT must(slots(:'other') = 2, format('the other database holds %s slots, not 2', slots(:'other')));

-- Dropped: its slots are free.
DROP DATABASE regress_290_other;
SELECT must(slots(:'other') = 0, format('the dropped database still holds %s slots', slots(:'other')));

-- A failed drop (a connection open, no FORCE) keeps them.
CREATE DATABASE regress_290_other;
SELECT oid AS other FROM pg_database WHERE datname = 'regress_290_other' \gset
SELECT dblink_connect('kept', :'conninfo') \g /dev/null
SELECT dblink_exec('kept', $$
    CREATE EXTENSION jsonb_delta;
    CREATE EXTENSION pg_tviews;
    CREATE TABLE tb_a (pk_a int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v text);
    SELECT tviews.pg_tviews_create('tv_a', 'SELECT pk_a, id, jsonb_build_object(''v'', v) AS data FROM tb_a');
    INSERT INTO tb_a (pk_a, v) VALUES (1, 'x');
$$) \g /dev/null
\set ON_ERROR_STOP off
DROP DATABASE regress_290_other;
\set ON_ERROR_STOP on
SELECT must(:'LAST_ERROR_SQLSTATE' = '55006', 'the drop did not fail: ' || :'LAST_ERROR_SQLSTATE');
SELECT must(slots(:'other') = 1, format('a failed drop left %s slots, not 1', slots(:'other')));
SELECT dblink_disconnect('kept') \g /dev/null
DROP DATABASE regress_290_other WITH (FORCE);
SELECT must(slots(:'other') = 0, 'FORCE left slots');

DROP EXTENSION pg_tviews CASCADE;
\echo 'drop database stats: PASS'

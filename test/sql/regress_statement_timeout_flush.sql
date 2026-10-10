-- A statement cancelled while its refreshes are applied leaves nothing behind.
--
-- statement_timeout fires during the flush that runs at the end of a writing
-- statement. The statement fails and its writes roll back, at top level and in a
-- savepoint; the next statements of the session refresh normally, nothing stays
-- queued, and the TVIEW equals its view.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_statement_timeout_flush.sql
-- expect-output: statement timeout: all fresh

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

-- Sleeps when the flag is set, so only the flush under test is slow. Its result
-- does not depend on the flag, so it is declared as reading no table.
CREATE TABLE slow_flag (on_ boolean);
INSERT INTO slow_flag VALUES (false);
CREATE FUNCTION public.slow(v int) RETURNS int LANGUAGE plpgsql STABLE AS $$
BEGIN
    IF (SELECT on_ FROM public.slow_flag) THEN
        PERFORM pg_sleep(0.5);
    END IF;
    RETURN v;
END $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n int);
INSERT INTO tb_item SELECT g, gen_random_uuid(), 0 FROM generate_series(1, 3) g;
SELECT pg_tviews_create_or_replace('tv_item',
  $q$SELECT pk_item, id, jsonb_build_object('id', id, 'n', public.slow(n)) AS data FROM tb_item$q$,
  options => '{"function_reads": {"public.slow(integer)": []}}');
UPDATE slow_flag SET on_ = true;

-- Top level: the UPDATE fails in its flush, and rolls back.
SET statement_timeout = '200ms';
DO $$
BEGIN
    UPDATE tb_item SET n = 1;
    RAISE EXCEPTION 'the flush was not cancelled';
EXCEPTION WHEN query_canceled THEN
    NULL;
END $$;
RESET statement_timeout;
UPDATE slow_flag SET on_ = false;
SELECT assert_fresh('tv_item', 'pk_item', 'a write cancelled during its flush');
UPDATE tb_item SET n = 2;
SELECT assert_fresh('tv_item', 'pk_item', 'the write after a cancelled flush');

-- In a transaction block: cancel inside a savepoint, roll back to it, go on.
UPDATE slow_flag SET on_ = true;
BEGIN;
UPDATE tb_item SET n = 3 WHERE pk_item = 1;
SAVEPOINT s;
SET LOCAL statement_timeout = '200ms';
\set ON_ERROR_STOP off
UPDATE tb_item SET n = 4;
\set ON_ERROR_STOP on
ROLLBACK TO SAVEPOINT s;
SET LOCAL statement_timeout = 0;
UPDATE slow_flag SET on_ = false;
UPDATE tb_item SET n = 5 WHERE pk_item = 2;
COMMIT;
SELECT assert_fresh('tv_item', 'pk_item', 'a transaction whose flush was cancelled in a savepoint');

\echo 'statement timeout: all fresh'

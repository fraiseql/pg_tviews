-- A TRUNCATE inside a writing statement leaves the flush to that statement.
--
-- A statement nested in a write (a trigger's) does not flush at its end: the
-- enclosing statement flushes once, seeing every key. A TRUNCATE run by a
-- trigger is such a statement; its TVIEWs are refreshed when the enclosing
-- statement flushes, and are fresh afterwards.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_truncate_in_writing_statement.sql
-- expect-output: truncate in writing statement: all fresh

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE TABLE tb_stage (pk_stage int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v text);
CREATE TABLE tb_batch (pk_batch int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), state text);
INSERT INTO tb_stage SELECT g, gen_random_uuid(), 'v' || g FROM generate_series(1, 5) g;
INSERT INTO tb_batch VALUES (1, DEFAULT, 'open');
SELECT pg_tviews_create('tv_stage',
  $q$SELECT pk_stage, id, jsonb_build_object('v', v) AS data FROM tb_stage$q$);
SELECT pg_tviews_create('tv_batch',
  $q$SELECT pk_batch, id, jsonb_build_object('state', state) AS data FROM tb_batch$q$);

-- Closing a batch empties the staging table, then restages one row.
CREATE FUNCTION close_batch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    TRUNCATE tb_stage;
    INSERT INTO tb_stage VALUES (100, DEFAULT, 'restaged');
    RETURN NULL;
END $$;
CREATE TRIGGER close_batch AFTER UPDATE ON tb_batch FOR EACH ROW
    WHEN (NEW.state = 'closed') EXECUTE FUNCTION close_batch();

DO $$
DECLARE before bigint := (pg_tviews_queue_stats()->>'flushes')::bigint;
BEGIN
    UPDATE tb_batch SET state = 'closed';
    IF (pg_tviews_queue_stats()->>'flushes')::bigint - before IS DISTINCT FROM 1 THEN
        RAISE EXCEPTION 'expected one flush for the UPDATE, got %',
            (pg_tviews_queue_stats()->>'flushes')::bigint - before;
    END IF;
END $$;
SELECT assert_fresh('tv_stage', 'pk_stage', 'a TRUNCATE run by a trigger of an UPDATE');
SELECT assert_fresh('tv_batch', 'pk_batch', 'a TRUNCATE run by a trigger of an UPDATE');

\echo 'truncate in writing statement: all fresh'

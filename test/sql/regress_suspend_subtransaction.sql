-- Suspension follows subtransactions, and COMMIT of a failed transaction runs
-- nothing.
--
-- pg_tviews_suspend_triggers() inside a savepoint that is rolled back is undone
-- with it: the next write refreshes. A transaction that suspended, wrote, and
-- then failed in a savepoint it did not roll back to ends with ROLLBACK when the
-- client says COMMIT; pg_tviews must not run its catch-up or flush in the
-- aborted transaction.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_suspend_subtransaction.sql
-- expect-output: suspend subtransaction: all fresh

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n int);
INSERT INTO tb_item SELECT g, gen_random_uuid(), 0 FROM generate_series(1, 3) g;
SELECT pg_tviews_create('tv_item',
  $q$SELECT pk_item, id, jsonb_build_object('id', id, 'n', n) AS data FROM tb_item$q$);

-- A suspension rolled back with its savepoint.
BEGIN;
SAVEPOINT s;
SELECT pg_tviews_suspend_triggers();
ROLLBACK TO SAVEPOINT s;
UPDATE tb_item SET n = 1;
SELECT assert_fresh('tv_item', 'pk_item', 'a write after a suspension rolled back with its savepoint');
COMMIT;

-- A resume rolled back with its savepoint: still suspended, caught up at COMMIT.
BEGIN;
SELECT pg_tviews_suspend_triggers();
UPDATE tb_item SET n = 2;
SAVEPOINT s;
SELECT pg_tviews_resume_triggers();
ROLLBACK TO SAVEPOINT s;
UPDATE tb_item SET n = 3 WHERE pk_item = 1;
COMMIT;
SELECT assert_fresh('tv_item', 'pk_item', 'COMMIT after a resume rolled back with its savepoint');

-- COMMIT of a transaction left failed: the server rolls it back, and pg_tviews
-- runs no SQL in it.
BEGIN;
SELECT pg_tviews_suspend_triggers();
UPDATE tb_item SET n = 4;
SAVEPOINT s;
\set ON_ERROR_STOP off
SELECT 1 / 0;
\set ON_ERROR_STOP on
COMMIT;
SELECT assert_fresh('tv_item', 'pk_item', 'COMMIT of a transaction that failed while suspended');
DO $$ BEGIN
    IF (SELECT max(n) FROM tb_item) <> 3 THEN
        RAISE EXCEPTION 'the failed transaction was not rolled back';
    END IF;
END $$;

\echo 'suspend subtransaction: all fresh'

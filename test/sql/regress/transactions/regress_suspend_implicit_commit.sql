-- A transaction that suspended refresh, wrote, and commits implicitly fails with
-- 55000 (architecture review A1).
--
-- An explicit COMMIT while suspended catches up first (the ProcessUtility hook
-- refreshes what the suspended writes touched). An implicit commit reaches no hook:
-- a query string of several statements (one implicit transaction), or a COMMIT
-- inside a procedure (SPI_commit). It used to commit the stale TVIEWs with a
-- WARNING; it must fail instead, as work left in the queue does, and roll back.
-- A procedure that commits while suspended therefore aborts the CALL at that
-- COMMIT: resume before committing.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_suspend_implicit_commit.sql
-- expect-output: suspend_implicit_commit: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT
);
INSERT INTO tb_user (name) VALUES ('ann'), ('bob');
CREATE TABLE tv_user AS SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user;

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'A1 FAIL: %', msg; END IF; END $$;

CREATE PROCEDURE write_suspended(resume boolean) LANGUAGE plpgsql AS $$
BEGIN
    PERFORM tviews.pg_tviews_suspend_triggers();
    UPDATE tb_user SET name = 'proc' WHERE pk_user = 2;
    IF resume THEN
        PERFORM tviews.pg_tviews_resume_triggers();
    END IF;
    COMMIT;
END $$;

-- One query string, one implicit transaction: the UPDATE must not commit.
\set ON_ERROR_STOP off
SELECT tviews.pg_tviews_suspend_triggers() \; UPDATE tb_user SET name = 'multi' WHERE pk_user = 1;
\set ON_ERROR_STOP on
SELECT must(:'LAST_ERROR_SQLSTATE' = '55000',
            'an implicit commit while suspended gave SQLSTATE ' || :'LAST_ERROR_SQLSTATE' || ', not 55000');
SELECT must((SELECT name FROM tb_user WHERE pk_user = 1) = 'ann', 'the suspended write committed');

-- A procedure's COMMIT while suspended: 55000, the CALL aborts.
\set ON_ERROR_STOP off
CALL write_suspended(false);
\set ON_ERROR_STOP on
SELECT must(:'LAST_ERROR_SQLSTATE' = '55000',
            'a procedure COMMIT while suspended gave SQLSTATE ' || :'LAST_ERROR_SQLSTATE' || ', not 55000');
SELECT must((SELECT name FROM tb_user WHERE pk_user = 2) = 'bob',
            'the suspended write in the procedure committed');

-- Resuming before the COMMIT is the way: it commits, and the TVIEW is fresh.
CALL write_suspended(true);
DO $$ BEGIN
  IF (SELECT data->>'name' FROM tv_user WHERE pk_user = 2) IS DISTINCT FROM 'proc' THEN
    RAISE EXCEPTION 'A1 FAIL: a resumed procedure write left tv_user stale';
  END IF;
END $$;

\echo 'suspend_implicit_commit: PASS'

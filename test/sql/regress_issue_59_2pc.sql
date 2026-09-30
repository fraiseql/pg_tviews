-- Regression test for issue #59: two-phase commit with pending TVIEW refreshes.
--
-- PREPARE TRANSACTION was rejected whenever refreshes were still queued. The refresh
-- writes are ordinary writes of the transaction, so flushing the queue before PREPARE
-- (as explicit COMMIT already does) makes them part of the prepared transaction:
-- COMMIT PREPARED applies them, ROLLBACK PREPARED discards them, with the standard
-- commands and no pg_tviews-specific API.
--
-- A statement's AFTER STATEMENT trigger flushes its own refreshes, so a pending queue at
-- PREPARE is produced with pg_tviews_cascade(), which enqueues without a statement.
--
-- Skipped when the cluster has max_prepared_transactions = 0.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_59_2pc.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

SELECT current_setting('max_prepared_transactions')::int = 0 AS no_2pc \gset
\if :no_2pc
  \echo 'SKIP: max_prepared_transactions = 0'
  \quit
\endif

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_author (
    pk_author BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name      TEXT
);
CREATE TABLE tb_post (
    pk_post   BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author BIGINT NOT NULL,
    title     TEXT
);
INSERT INTO tb_author (name) VALUES ('ann');
INSERT INTO tb_post (fk_author, title) VALUES (1, 'p1');
CREATE TABLE tv_post AS
SELECT p.pk_post, p.id, p.fk_author,
       jsonb_build_object('title', p.title, 'author', a.name) AS data
FROM tb_post p JOIN tb_author a ON a.pk_author = p.fk_author;

-- Leave the refresh queued: change the author without its triggers, then enqueue the
-- dependent refresh through the API.
CREATE FUNCTION rename_author_queued(new_name TEXT) RETURNS BIGINT LANGUAGE plpgsql AS $$
BEGIN
    ALTER TABLE tb_author DISABLE TRIGGER USER;
    UPDATE tb_author SET name = new_name WHERE pk_author = 1;
    ALTER TABLE tb_author ENABLE TRIGGER USER;
    PERFORM pg_tviews_cascade('tb_author'::regclass, 1);
    RETURN (pg_tviews_queue_stats()->>'queue_size')::bigint;
END $$;

-- ========================================================================
-- Cycle 1: PREPARE with a pending refresh, then COMMIT PREPARED
-- ========================================================================
BEGIN;
SELECT rename_author_queued('bob') AS queued \gset
PREPARE TRANSACTION 'pg_tviews_59_commit';
COMMIT PREPARED 'pg_tviews_59_commit';

SELECT :queued < 1 AS nothing_pending \gset
\if :nothing_pending
  DO $$ BEGIN RAISE EXCEPTION '#59 setup FAIL: no refresh was pending at PREPARE'; END $$;
\endif

DO $$ BEGIN
  IF (SELECT data->>'author' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 'bob' THEN
    RAISE EXCEPTION '#59 FAIL: COMMIT PREPARED did not apply the pending refresh (got %)',
      (SELECT data->>'author' FROM tv_post WHERE pk_post = 1);
  END IF;
END $$;

-- ========================================================================
-- Cycle 2: PREPARE with a pending refresh, then ROLLBACK PREPARED
-- ========================================================================
BEGIN;
SELECT rename_author_queued('cid');
PREPARE TRANSACTION 'pg_tviews_59_rollback';
ROLLBACK PREPARED 'pg_tviews_59_rollback';

DO $$ BEGIN
  IF (SELECT data->>'author' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 'bob'
     OR (SELECT name FROM tb_author WHERE pk_author = 1) IS DISTINCT FROM 'bob' THEN
    RAISE EXCEPTION '#59 FAIL: ROLLBACK PREPARED left tv_post=% tb_author=%',
      (SELECT data->>'author' FROM tv_post WHERE pk_post = 1),
      (SELECT name FROM tb_author WHERE pk_author = 1);
  END IF;
END $$;

-- ========================================================================
-- Cycle 3: ordinary DML in a prepared transaction, and a clean queue afterwards
-- ========================================================================
BEGIN;
UPDATE tb_post SET title = 'p2' WHERE pk_post = 1;
PREPARE TRANSACTION 'pg_tviews_59_dml';
COMMIT PREPARED 'pg_tviews_59_dml';

DO $$ BEGIN
  IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 'p2' THEN
    RAISE EXCEPTION '#59 FAIL: DML in a prepared transaction did not reach tv_post';
  END IF;
  IF (pg_tviews_queue_stats()->>'queue_size')::int <> 0 THEN
    RAISE EXCEPTION '#59 FAIL: queue not empty after the prepared transactions';
  END IF;
END $$;

-- ========================================================================
-- Cycle 4: the first write to an empty TVIEW does not lock readers out
-- ========================================================================
-- An empty UNLOGGED TVIEW whose view gains rows looks like a crash-reset table. Its
-- repopulation used TRUNCATE, whose ACCESS EXCLUSIVE lock a prepared transaction then
-- held until COMMIT PREPARED, blocking every reader of the TVIEW.
CREATE TABLE tb_note (
    pk_note BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    body    TEXT
);
CREATE TABLE tv_note AS
SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM tb_note;

BEGIN;
INSERT INTO tb_note (body) VALUES ('n1');
PREPARE TRANSACTION 'pg_tviews_59_first_write';
SELECT EXISTS (SELECT 1 FROM pg_locks
               WHERE pid IS NULL AND relation = 'tv_note'::regclass
                 AND mode = 'AccessExclusiveLock') AS exclusive_held \gset
COMMIT PREPARED 'pg_tviews_59_first_write';

\if :exclusive_held
  DO $$ BEGIN RAISE EXCEPTION '#59 FAIL: the prepared transaction held ACCESS EXCLUSIVE on tv_note'; END $$;
\endif

DO $$ BEGIN
  IF (SELECT count(*) FROM tv_note) <> 1 THEN
    RAISE EXCEPTION '#59 FAIL: first write to an empty TVIEW not applied';
  END IF;
END $$;

-- Regression test for issue #56 (Phase 2/3): savepoint safety of the patch map.
--
-- The direct-patch map snapshots/restores in lockstep with the refresh queue on
-- SAVEPOINT / ROLLBACK TO. A change captured inside a rolled-back savepoint must
-- not survive; the final tview reflects only the pre-savepoint change. This holds
-- whether the flush recomputes (Phase 2) or applies a direct patch (Phase 3),
-- because the tview write is itself transactional.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_56_savepoint.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_user CASCADE;
DROP VIEW  IF EXISTS v_user CASCADE;
DROP TABLE IF EXISTS tb_user CASCADE;

CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    bio     TEXT
);
INSERT INTO tb_user (bio) VALUES ('initial');

SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('bio', bio) AS data
    FROM tb_user
$TVIEW$);

-- (1) ROLLBACK TO discards the savepoint-local change.
BEGIN;
  UPDATE tb_user SET bio = 'before-sp' WHERE pk_user = 1;
  SAVEPOINT sp1;
  UPDATE tb_user SET bio = 'inside-sp' WHERE pk_user = 1;
  ROLLBACK TO sp1;
COMMIT;

DO $$ BEGIN
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'before-sp' THEN
    RAISE EXCEPTION '#56 savepoint FAIL: expected before-sp, got %',
      (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1);
  END IF;
END $$;

-- (2) RELEASE SAVEPOINT keeps the savepoint-local change.
BEGIN;
  UPDATE tb_user SET bio = 'pre-release' WHERE pk_user = 1;
  SAVEPOINT sp2;
  UPDATE tb_user SET bio = 'kept' WHERE pk_user = 1;
  RELEASE SAVEPOINT sp2;
COMMIT;

DO $$ BEGIN
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'kept' THEN
    RAISE EXCEPTION '#56 savepoint FAIL: expected kept, got %',
      (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1);
  END IF;
END $$;

-- (3) Full transaction ABORT clears the patch map and leaves the tview unchanged.
BEGIN;
  UPDATE tb_user SET bio = 'aborted' WHERE pk_user = 1;
ROLLBACK;

DO $$ BEGIN
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'kept' THEN
    RAISE EXCEPTION '#56 abort FAIL: expected kept (unchanged), got %',
      (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1);
  END IF;
END $$;

SELECT 'issue #56 savepoint: PASS' AS result;

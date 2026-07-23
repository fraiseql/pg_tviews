-- Regression test for issue #56 (Phase 5, scenario #19): jsonb_delta NOT installed.
--
-- The direct-patch fast path requires the jsonb_smart_patch_* primitives, so with
-- jsonb_delta absent capture must decline (no patch captured, none applied) and the
-- full-replacement recompute path produces correct data.
--
-- Deliberately does NOT create the jsonb_delta extension (the runner still executes
-- *fallback* tests when jsonb_delta is unavailable in the cluster).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_56_fallback_no_delta.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION pg_tviews;   -- no jsonb_delta on purpose

DROP TABLE IF EXISTS tv_user CASCADE;
DROP VIEW  IF EXISTS v_user CASCADE;
DROP TABLE IF EXISTS tb_user CASCADE;

CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT, bio TEXT
);
INSERT INTO tb_user (name, bio) VALUES ('Alice', 'b0');

SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data
    FROM tb_user
$TVIEW$);

-- An otherwise-eligible single-column UPDATE: capture must decline (no jsonb_delta),
-- neither captured nor applied, and the full-replacement path must produce the row.
DO $$
DECLARE cap0 bigint; app0 bigint;
BEGIN
  cap0 := (pg_tviews_queue_stats()->>'direct_patch_captured')::bigint;
  app0 := (pg_tviews_queue_stats()->>'direct_patches_applied')::bigint;

  UPDATE tb_user SET bio = 'b1' WHERE pk_user = 1;

  IF (pg_tviews_queue_stats()->>'direct_patch_captured')::bigint <> cap0 THEN
    RAISE EXCEPTION '#56 no-delta FAIL: a patch was captured without jsonb_delta';
  END IF;
  IF (pg_tviews_queue_stats()->>'direct_patches_applied')::bigint <> app0 THEN
    RAISE EXCEPTION '#56 no-delta FAIL: a patch was applied without jsonb_delta';
  END IF;
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'b1'
     OR (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) <> 'Alice' THEN
    RAISE EXCEPTION '#56 no-delta FAIL: full-replacement recompute wrong (%)',
      (SELECT data FROM tv_user WHERE pk_user = 1);
  END IF;
END $$;

SELECT 'issue #56 fallback (no jsonb_delta): PASS' AS result;

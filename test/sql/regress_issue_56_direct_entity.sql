-- Regression test for issue #56 (Phase 3): flush-time direct patch, direct entity.
--
-- An eligible UPDATE patches tv_<entity> directly via jsonb_smart_patch_scalar with
-- ZERO backing-view queries; untouched JSONB keys stay byte-identical; missing rows,
-- GUC-off, and ineligible (poisoning) changes all fall back to recompute.
--
-- Standalone tv_user (no parent) so "view_recomputes unchanged" isolates the
-- direct-entity path.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_56_direct_entity.sql

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
    name    TEXT,
    bio     TEXT,
    status  TEXT
);
INSERT INTO tb_user (name, bio, status)
     VALUES ('Alice', 'a0', 'active'), ('Bob', 'b0', 'active'), ('Carol', 'c0', 'active');

SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data
    FROM tb_user
$TVIEW$);

-- Counter-delta helper over (applied, fallbacks, view_recomputes).
CREATE TEMP TABLE _c(applied bigint, fallbacks bigint, recomputes bigint);
INSERT INTO _c
SELECT (s->>'direct_patches_applied')::bigint,
       (s->>'direct_patch_fallbacks')::bigint,
       (s->>'view_recomputes')::bigint
FROM (SELECT pg_tviews_queue_stats() AS s) x;

CREATE FUNCTION _c_assert(exp_applied bigint, exp_fb bigint, exp_rc bigint, label text)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE s jsonb; a bigint; f bigint; r bigint; pa bigint; pf bigint; pr bigint;
BEGIN
  s := pg_tviews_queue_stats();
  a := (s->>'direct_patches_applied')::bigint;
  f := (s->>'direct_patch_fallbacks')::bigint;
  r := (s->>'view_recomputes')::bigint;
  SELECT applied, fallbacks, recomputes INTO pa, pf, pr FROM _c;
  IF a - pa <> exp_applied OR f - pf <> exp_fb OR r - pr <> exp_rc THEN
    RAISE EXCEPTION '#56 [%] counters: applied Δ%/exp%, fallback Δ%/exp%, recompute Δ%/exp%',
      label, a - pa, exp_applied, f - pf, exp_fb, r - pr, exp_rc;
  END IF;
  UPDATE _c SET applied = a, fallbacks = f, recomputes = r;
END $$;

-- ── Cycle 1: single eligible column, direct patch, merge semantics ───────────
-- Baseline for updated_at bump.
CREATE TEMP TABLE _ts AS SELECT updated_at AS u FROM tv_user WHERE pk_user = 1;
SELECT pg_sleep(0.02);

UPDATE tb_user SET bio = 'a1' WHERE pk_user = 1;
SELECT _c_assert(1, 0, 0, 'single eligible bio: patched, no recompute');

DO $$ BEGIN
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'a1' THEN
    RAISE EXCEPTION '#56 FAIL: bio not patched (got %)',
      (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1);
  END IF;
  -- Merge, not replace: the untouched `name` key survives.
  IF (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) <> 'Alice' THEN
    RAISE EXCEPTION '#56 FAIL: merge clobbered name (got %)',
      (SELECT data->>'name' FROM tv_user WHERE pk_user = 1);
  END IF;
  -- Exactly the two expected keys remain.
  IF (SELECT count(*) FROM jsonb_object_keys((SELECT data FROM tv_user WHERE pk_user=1))) <> 2 THEN
    RAISE EXCEPTION '#56 FAIL: unexpected key set %',
      (SELECT data FROM tv_user WHERE pk_user = 1);
  END IF;
  -- updated_at bumped.
  IF (SELECT updated_at FROM tv_user WHERE pk_user = 1) <= (SELECT u FROM _ts) THEN
    RAISE EXCEPTION '#56 FAIL: updated_at not bumped by direct patch';
  END IF;
END $$;

-- Multi-column eligible in one row ⇒ one applied row, no recompute.
UPDATE tb_user SET bio = 'a2', name = 'Alice2' WHERE pk_user = 1;
SELECT _c_assert(1, 0, 0, 'multi-column eligible: one applied');
DO $$ BEGIN
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user=1) <> 'a2'
     OR (SELECT data->>'name' FROM tv_user WHERE pk_user=1) <> 'Alice2' THEN
    RAISE EXCEPTION '#56 FAIL: multi-column patch wrong (%)',
      (SELECT data FROM tv_user WHERE pk_user = 1);
  END IF;
END $$;

-- ── Cycle 2: bulk grouping + batch chunking ──────────────────────────────────
-- Same new value for many rows ⇒ one chain group; 3 rows applied, 0 recomputes.
UPDATE tb_user SET bio = 'shared' WHERE pk_user IN (1, 2, 3);
SELECT _c_assert(3, 0, 0, 'bulk same-value: 3 applied');
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_user WHERE data->>'bio' = 'shared') <> 3 THEN
    RAISE EXCEPTION '#56 FAIL: bulk same-value patch missed rows';
  END IF;
END $$;

-- Distinct new value per row (different chains) under a small batch_size.
SET pg_tviews.batch_size = 2;
UPDATE tb_user SET bio = 'v_' || pk_user::text WHERE pk_user IN (1, 2, 3);
SELECT _c_assert(3, 0, 0, 'bulk distinct-value: 3 applied under batch_size=2');
RESET pg_tviews.batch_size;
DO $$ BEGIN
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user=1) <> 'v_1'
     OR (SELECT data->>'bio' FROM tv_user WHERE pk_user=2) <> 'v_2'
     OR (SELECT data->>'bio' FROM tv_user WHERE pk_user=3) <> 'v_3' THEN
    RAISE EXCEPTION '#56 FAIL: distinct-value bulk patch wrong';
  END IF;
END $$;

-- ── Cycle 3: fallbacks ───────────────────────────────────────────────────────
-- (a) Missing tview row ⇒ patch RETURNING empty ⇒ recompute (UPSERT re-creates it).
DELETE FROM tv_user WHERE pk_user = 2;
UPDATE tb_user SET bio = 'reborn' WHERE pk_user = 2;
SELECT _c_assert(0, 1, 1, 'missing row: fallback to recompute');
DO $$ BEGIN
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 2) <> 'reborn'
     OR (SELECT data->>'name' FROM tv_user WHERE pk_user = 2) <> 'Bob' THEN
    RAISE EXCEPTION '#56 FAIL: fallback recompute produced wrong row (%)',
      (SELECT data FROM tv_user WHERE pk_user = 2);
  END IF;
END $$;

-- (b) GUC off between capture and commit ⇒ recompute, not patch.
SET pg_tviews.direct_patch_enabled = off;
UPDATE tb_user SET bio = 'guc_off' WHERE pk_user = 1;
SELECT _c_assert(0, 0, 1, 'guc off: recompute');
SET pg_tviews.direct_patch_enabled = on;
DO $$ BEGIN
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'guc_off' THEN
    RAISE EXCEPTION '#56 FAIL: guc-off recompute wrong';
  END IF;
END $$;

-- (c) Ineligible mixed change (bio mapped + status unmapped) poisons ⇒ recompute.
UPDATE tb_user SET bio = 'mix', status = 'inactive' WHERE pk_user = 1;
SELECT _c_assert(0, 0, 1, 'mixed eligible+unmapped: poison to recompute');
DO $$ BEGIN
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'mix' THEN
    RAISE EXCEPTION '#56 FAIL: mixed-update recompute wrong';
  END IF;
END $$;

-- ── Cycle 4: zero-view-query proof for the eligible direct-entity path ────────
DO $$
DECLARE before_rc bigint; after_rc bigint;
BEGIN
  before_rc := (pg_tviews_queue_stats()->>'view_recomputes')::bigint;
  UPDATE tb_user SET bio = 'final' WHERE pk_user = 3;
  after_rc := (pg_tviews_queue_stats()->>'view_recomputes')::bigint;
  IF after_rc <> before_rc THEN
    RAISE EXCEPTION '#56 FAIL: eligible direct-entity UPDATE triggered % view recompute(s)',
      after_rc - before_rc;
  END IF;
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 3) <> 'final' THEN
    RAISE EXCEPTION '#56 FAIL: zero-recompute update produced wrong data';
  END IF;
END $$;

SELECT 'issue #56 direct entity: PASS' AS result;

-- Regression test for issue #56 (Phase 2): trigger diff + patch capture.
--
-- The row trigger captures a direct patch only for an eligible single-row UPDATE
-- (every changed column maps identity-style into the entity's own `data`, no
-- FK/PK/projected-column change). Observed via the session-cumulative
-- `direct_patch_captured` counter in pg_tviews_queue_stats(). Patches are captured
-- but NOT yet applied here (Phase 3) — the recompute path still produces the data.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_56_capture.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_post CASCADE;
DROP VIEW  IF EXISTS v_post CASCADE;
DROP TABLE IF EXISTS tv_user CASCADE;
DROP VIEW  IF EXISTS v_user CASCADE;
DROP TABLE IF EXISTS tb_post CASCADE;
DROP TABLE IF EXISTS tb_user CASCADE;

CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT,
    bio     TEXT,
    status  TEXT          -- unmapped base column (not in data, not projected)
);
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user INTEGER REFERENCES tb_user(pk_user),
    title   TEXT
);
INSERT INTO tb_user (name, bio, status) VALUES ('Alice', 'hello', 'active'), ('Bob', 'hi', 'active');
INSERT INTO tb_post (fk_user, title) VALUES (1, 'Original');

SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data
    FROM tb_user
$TVIEW$);
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT tb_post.pk_post, tb_post.id, tb_post.fk_user,
           jsonb_build_object('title', tb_post.title, 'author', v_user.data) AS data
    FROM tb_post LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
$TVIEW$);

-- Cycle 3: the kill-switch GUC exists and defaults to on.
DO $$ BEGIN
  IF current_setting('pg_tviews.direct_patch_enabled') <> 'on' THEN
    RAISE EXCEPTION '#56 FAIL: direct_patch_enabled default is % (expected on)',
      current_setting('pg_tviews.direct_patch_enabled');
  END IF;
END $$;

-- Delta-assertion helper over the session-cumulative capture counter.
CREATE TEMP TABLE _dp(prev bigint);
INSERT INTO _dp VALUES ((pg_tviews_queue_stats()->>'direct_patch_captured')::bigint);

CREATE FUNCTION _dp_assert(expected bigint, label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE cur bigint; prv bigint;
BEGIN
  cur := (pg_tviews_queue_stats()->>'direct_patch_captured')::bigint;
  SELECT prev INTO prv FROM _dp;
  IF cur - prv <> expected THEN
    RAISE EXCEPTION '#56 capture FAIL [%]: expected delta %, got % (prev=%, cur=%)',
      label, expected, cur - prv, prv, cur;
  END IF;
  UPDATE _dp SET prev = cur;
END $$;

-- (1) eligible single mapped column ⇒ captured.
UPDATE tb_user SET bio = 'b1' WHERE pk_user = 1;
SELECT _dp_assert(1, 'eligible single column (bio)');

-- (2) eligible single mapped column (name) ⇒ captured.
UPDATE tb_user SET name = 'Alice2' WHERE pk_user = 1;
SELECT _dp_assert(1, 'eligible single column (name)');

-- (3) eligible multi-column (both mapped) in one row ⇒ one capture.
UPDATE tb_user SET bio = 'b2', name = 'Alice3' WHERE pk_user = 1;
SELECT _dp_assert(1, 'eligible multi-column (bio+name)');

-- (4) FK-column change ⇒ NOT captured (membership change).
UPDATE tb_post SET fk_user = 2 WHERE pk_post = 1;
SELECT _dp_assert(0, 'fk-column update');

-- (5) mixed eligible + ineligible in one UPDATE ⇒ NOT captured.
UPDATE tb_post SET title = 'T2', fk_user = 1 WHERE pk_post = 1;
SELECT _dp_assert(0, 'mixed eligible+fk update');

-- (6) unmapped base column change ⇒ NOT captured (could be a filter/computed input).
UPDATE tb_user SET status = 'inactive' WHERE pk_user = 1;
SELECT _dp_assert(0, 'unmapped column (status)');

-- (7) INSERT ⇒ NOT captured (no OLD row; membership change).
INSERT INTO tb_user (name, bio, status) VALUES ('Carol', 'yo', 'active');
SELECT _dp_assert(0, 'insert');

-- (8) DELETE ⇒ NOT captured.
DELETE FROM tb_user WHERE name = 'Carol';
SELECT _dp_assert(0, 'delete');

-- (9) GUC off ⇒ eligible UPDATE NOT captured (kill-switch honoured at capture time).
SET pg_tviews.direct_patch_enabled = off;
UPDATE tb_user SET bio = 'b3' WHERE pk_user = 1;
SELECT _dp_assert(0, 'guc off suppresses capture');
SET pg_tviews.direct_patch_enabled = on;

-- Sanity: recompute path still produces correct data in Phase 2 (patches unused).
DO $$ BEGIN
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'b3' THEN
    RAISE EXCEPTION '#56 FAIL: tv_user.bio not refreshed by recompute (got %)',
      (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1);
  END IF;
END $$;

SELECT 'issue #56 capture: PASS' AS result;

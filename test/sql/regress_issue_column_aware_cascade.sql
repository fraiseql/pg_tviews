-- Regression test: column-aware cascade refresh.
--
-- An UPDATE to a base-table column that a dependent TVIEW does not project cannot
-- change any of that TVIEW's rows, so the cascade recompute must be SKIPPED. The
-- set of relevant source columns is derived at create time from PostgreSQL's own
-- column-level pg_depend records on the backing view v_<entity>.
--
-- Lean read model: tv_post embeds only the author's username/full_name (NOT bio).
--   - UPDATE tb_user.bio      → tv_post cascade SKIPPED (0 recomputes), no staleness
--   - UPDATE tb_user.username → tv_post cascade RUNS, embed reflects the change
--   - UPDATE tb_user.fk-side / membership columns still cascade (safe default)
--   - Direct entity (tv_user) always refreshes regardless
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_column_aware_cascade.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_post CASCADE;  DROP VIEW IF EXISTS v_post CASCADE;
DROP TABLE IF EXISTS tv_user CASCADE;  DROP VIEW IF EXISTS v_user CASCADE;
DROP TABLE IF EXISTS tb_post CASCADE;
DROP TABLE IF EXISTS tb_user CASCADE;

CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    username TEXT, full_name TEXT, bio TEXT
);
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author INTEGER REFERENCES tb_user(pk_user), title TEXT
);

INSERT INTO tb_user (username, full_name, bio)
    VALUES ('alice', 'Alice A', 'a-bio'), ('zoe', 'Zoe Z', 'z-bio');
INSERT INTO tb_post (fk_author, title) VALUES (1, 'P1'), (1, 'P2'), (1, 'P3');

-- tv_user embeds everything (incl. bio).
SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id,
           jsonb_build_object('username', username, 'full_name', full_name, 'bio', bio) AS data
    FROM tb_user
$TVIEW$);

-- Lean tv_post: embeds ONLY username + full_name of the author (NOT bio),
-- built from base tb_user columns via a direct JOIN.
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT p.pk_post, p.id, p.fk_author, u.id AS author_id,
           jsonb_build_object('title', p.title,
               'author', jsonb_build_object(
                   'username', u.username, 'full_name', u.full_name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author
$TVIEW$);

CREATE FUNCTION _rc() RETURNS bigint LANGUAGE sql AS
  $$ SELECT (pg_tviews_queue_stats()->>'view_recomputes')::bigint $$;

-- ── Case 1: UPDATE a NON-projected column (bio) → cascade to tv_post SKIPPED ──
DO $$
DECLARE rc0 bigint;
BEGIN
  rc0 := _rc();
  UPDATE tb_user SET bio = 'a-bio-CHANGED' WHERE pk_user = 1;

  -- No tv_post recompute: bio is not part of tv_post's projection.
  IF _rc() <> rc0 THEN
    RAISE EXCEPTION 'column-aware FAIL: bio update triggered % recompute(s) (expected 0)',
      _rc() - rc0;
  END IF;
  -- Direct entity still refreshed.
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'a-bio-CHANGED' THEN
    RAISE EXCEPTION 'column-aware FAIL: tv_user.bio not refreshed';
  END IF;
  -- tv_post is (correctly) unchanged and NOT stale — it never embedded bio.
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'username' = 'alice') <> 3 THEN
    RAISE EXCEPTION 'column-aware FAIL: tv_post author.username unexpectedly changed';
  END IF;
END $$;

-- ── Case 2: UPDATE a projected column (username) → cascade RUNS, embed fresh ──
DO $$
DECLARE rc0 bigint;
BEGIN
  rc0 := _rc();
  UPDATE tb_user SET username = 'alice2' WHERE pk_user = 1;

  IF _rc() = rc0 THEN
    RAISE EXCEPTION 'column-aware FAIL: username update did not cascade (0 recomputes)';
  END IF;
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'username' = 'alice2') <> 3 THEN
    RAISE EXCEPTION 'column-aware FAIL: username change not reflected in tv_post.author (% of 3)',
      (SELECT count(*) FROM tv_post WHERE data->'author'->>'username' = 'alice2');
  END IF;
END $$;

-- ── Case 3: UPDATE full_name (also projected) → cascade RUNS ──────────────────
DO $$
DECLARE rc0 bigint;
BEGIN
  rc0 := _rc();
  UPDATE tb_user SET full_name = 'Alice Prime' WHERE pk_user = 1;
  IF _rc() = rc0 THEN
    RAISE EXCEPTION 'column-aware FAIL: full_name update did not cascade';
  END IF;
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'full_name' = 'Alice Prime') <> 3 THEN
    RAISE EXCEPTION 'column-aware FAIL: full_name change not reflected in tv_post.author';
  END IF;
END $$;

-- ── Case 4: mixed update (projected + non-projected) → cascade RUNS (any hit) ─
DO $$
DECLARE rc0 bigint;
BEGIN
  rc0 := _rc();
  UPDATE tb_user SET username = 'alice3', bio = 'a-bio-3' WHERE pk_user = 1;
  IF _rc() = rc0 THEN
    RAISE EXCEPTION 'column-aware FAIL: mixed update (username+bio) did not cascade';
  END IF;
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'username' = 'alice3') <> 3 THEN
    RAISE EXCEPTION 'column-aware FAIL: mixed update did not refresh username embed';
  END IF;
END $$;

-- ── Case 5: byte-identity — after all edits, fast path == forced full recompute ─
DO $$
DECLARE fast jsonb; rec jsonb;
BEGIN
  SELECT jsonb_agg(data ORDER BY pk_post) INTO fast FROM tv_post;
  PERFORM pg_tviews_refresh('post');
  SELECT jsonb_agg(data ORDER BY pk_post) INTO rec FROM tv_post;
  IF fast <> rec THEN
    RAISE EXCEPTION 'column-aware FAIL: tv_post not byte-identical to a forced recompute';
  END IF;
END $$;

SELECT 'column-aware cascade: PASS' AS result;

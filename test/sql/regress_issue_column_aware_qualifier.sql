-- Hardening: column-aware refresh must NOT skip a cascade for a source column
-- that affects the tview's output without appearing verbatim as a projected JSON
-- value. The relevant-column set is derived from column-level pg_depend on the
-- backing view, which records EVERY referenced column (SELECT list, expressions,
-- CASE, WHERE, JOIN/GROUP/ORDER) — so this is exactly where a naive SELECT-text
-- parse would create a silent-staleness hole. These are the "too narrow
-- source_columns ⇒ missed refresh" cases; each must still cascade.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_column_aware_qualifier.sql

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
    username TEXT, active BOOLEAN DEFAULT true, tier INTEGER DEFAULT 0, bio TEXT
);
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author INTEGER REFERENCES tb_user(pk_user), title TEXT
);

INSERT INTO tb_user (username, active, tier, bio)
    VALUES ('alice', true, 1, 'a-bio'), ('zoe', true, 2, 'z-bio');
INSERT INTO tb_post (fk_author, title) VALUES (1, 'P1'), (1, 'P2'), (1, 'P3');

SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id,
           jsonb_build_object('username', username, 'active', active, 'tier', tier, 'bio', bio) AS data
    FROM tb_user
$TVIEW$);

-- tv_post embeds author fields that reference tb_user columns only INDIRECTLY:
--   display  ← upper(u.username)                 (column inside an expression)
--   status   ← CASE WHEN u.active THEN … END     (column inside a CASE condition)
--   rank     ← u.tier * 10                        (column inside arithmetic)
-- and does NOT reference u.bio at all. A naive "which columns are JSON values"
-- parse would miss username/active/tier; column-level pg_depend must catch them.
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT p.pk_post, p.id, p.fk_author, u.id AS author_id,
           jsonb_build_object('title', p.title,
               'author', jsonb_build_object(
                   'display', upper(u.username),
                   'status',  CASE WHEN u.active THEN 'on' ELSE 'off' END,
                   'rank',    u.tier * 10)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author
$TVIEW$);

CREATE FUNCTION _rc() RETURNS bigint LANGUAGE sql AS
  $$ SELECT (pg_tviews_queue_stats()->>'view_recomputes')::bigint $$;

-- Baseline.
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_post
      WHERE data->'author'->>'display' = 'ALICE'
        AND data->'author'->>'status' = 'on'
        AND data->'author'->>'rank' = '10') <> 3 THEN
    RAISE EXCEPTION 'setup FAIL: initial derived author fields wrong';
  END IF;
END $$;

-- ── username referenced only via upper() → must cascade ──────────────────────
DO $$ BEGIN
  UPDATE tb_user SET username = 'alicia' WHERE pk_user = 1;
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'display' = 'ALICIA') <> 3 THEN
    RAISE EXCEPTION 'qualifier FAIL: expression column (upper(username)) wrongly skipped — % of 3',
      (SELECT count(*) FROM tv_post WHERE data->'author'->>'display' = 'ALICIA');
  END IF;
END $$;

-- ── active referenced only in a CASE condition → must cascade ────────────────
DO $$ BEGIN
  UPDATE tb_user SET active = false WHERE pk_user = 1;
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'status' = 'off') <> 3 THEN
    RAISE EXCEPTION 'qualifier FAIL: CASE-condition column (active) wrongly skipped — % of 3',
      (SELECT count(*) FROM tv_post WHERE data->'author'->>'status' = 'off');
  END IF;
END $$;

-- ── tier referenced only in arithmetic → must cascade ───────────────────────
DO $$ BEGIN
  UPDATE tb_user SET tier = 5 WHERE pk_user = 1;
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'rank' = '50') <> 3 THEN
    RAISE EXCEPTION 'qualifier FAIL: arithmetic column (tier) wrongly skipped — % of 3',
      (SELECT count(*) FROM tv_post WHERE data->'author'->>'rank' = '50');
  END IF;
END $$;

-- ── Negative control: bio is genuinely unreferenced → cascade SKIPPED ────────
DO $$
DECLARE rc0 bigint;
BEGIN
  rc0 := _rc();
  UPDATE tb_user SET bio = 'a-bio-CHANGED' WHERE pk_user = 1;
  IF _rc() <> rc0 THEN
    RAISE EXCEPTION 'qualifier FAIL: bio (unreferenced) still triggered % recompute(s)', _rc() - rc0;
  END IF;
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'a-bio-CHANGED' THEN
    RAISE EXCEPTION 'qualifier FAIL: tv_user.bio not refreshed';
  END IF;
END $$;

-- ── Byte-identity after all edits ────────────────────────────────────────────
DO $$
DECLARE fast jsonb; rec jsonb;
BEGIN
  SELECT jsonb_agg(data ORDER BY pk_post) INTO fast FROM tv_post;
  PERFORM pg_tviews_refresh('post');
  SELECT jsonb_agg(data ORDER BY pk_post) INTO rec FROM tv_post;
  IF fast <> rec THEN
    RAISE EXCEPTION 'qualifier FAIL: tv_post not byte-identical to a forced recompute';
  END IF;
END $$;

SELECT 'column-aware qualifier: PASS' AS result;

-- Regression test for issue #56 (Phase 4): cascade patch propagation to parents.
--
-- The headline win. A patched child entity embedded in parents via nested_object
-- dependencies propagates a DERIVED patch (child fields at the dependency path) —
-- the whole fan-out runs as grouped jsonb_smart_patch_nested UPDATEs with zero view
-- queries. Covers one-level, two-level, nested-patch preserves sibling array data,
-- byte-identity vs a forced recompute, and child-fallback → parent recompute.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_56_cascade.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_feed CASCADE;    DROP VIEW IF EXISTS v_feed CASCADE;
DROP TABLE IF EXISTS tv_post CASCADE;    DROP VIEW IF EXISTS v_post CASCADE;
DROP TABLE IF EXISTS tv_user CASCADE;    DROP VIEW IF EXISTS v_user CASCADE;
DROP TABLE IF EXISTS tb_feed CASCADE;
DROP TABLE IF EXISTS tb_comment CASCADE;
DROP TABLE IF EXISTS tb_post CASCADE;
DROP TABLE IF EXISTS tb_user CASCADE;

CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE, name TEXT, bio TEXT
);
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user INTEGER REFERENCES tb_user(pk_user), title TEXT
);
CREATE TABLE tb_comment (
    pk_comment INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_post INTEGER REFERENCES tb_post(pk_post), body TEXT
);
CREATE TABLE tb_feed (
    pk_feed INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_post INTEGER REFERENCES tb_post(pk_post), label TEXT
);

-- A second user (with no posts) keeps tv_user non-empty when Alice's row is
-- deleted in Cycle 4, so the crash-recovery truncation path stays dormant and the
-- genuine per-row fallback is exercised.
INSERT INTO tb_user (name, bio) VALUES ('Alice', 'author-bio'), ('Zoe', 'zoe-bio');
INSERT INTO tb_post (fk_user, title) VALUES (1, 'P1'), (1, 'P2'), (1, 'P3');
INSERT INTO tb_comment (fk_post, body) VALUES (1, 'c1'), (1, 'c2'), (2, 'c3');
INSERT INTO tb_feed (fk_post, label) VALUES (1, 'F1'), (2, 'F2'), (3, 'F3');

SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data
    FROM tb_user
$TVIEW$);
-- Post embeds its author (nested_object) AND aggregates its comments (array).
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT tb_post.pk_post, tb_post.id, tb_post.fk_user,
           jsonb_build_object(
               'title', tb_post.title,
               'author', v_user.data,
               'comments', COALESCE(jsonb_agg(
                   jsonb_build_object('body', c.body) ORDER BY c.pk_comment)
                   FILTER (WHERE c.pk_comment IS NOT NULL), '[]'::jsonb)
           ) AS data
    FROM tb_post
    LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
    LEFT JOIN tb_comment c ON c.fk_post = tb_post.pk_post
    GROUP BY tb_post.pk_post, tb_post.id, tb_post.fk_user, tb_post.title, v_user.data
$TVIEW$);
-- Feed embeds its post (nested_object) — two levels above user.
SELECT pg_tviews_create('tv_feed', $TVIEW$
    SELECT tb_feed.pk_feed, tb_feed.id, tb_feed.fk_post,
           jsonb_build_object('label', tb_feed.label, 'post', v_post.data) AS data
    FROM tb_feed LEFT JOIN v_post ON v_post.pk_post = tb_feed.fk_post
$TVIEW$);

-- Precondition: post is classified nested_object on the author dependency.
DO $$ BEGIN
  IF NOT (SELECT dependency_types @> ARRAY['nested_object']::text[]
          FROM pg_tview_meta WHERE entity = 'post') THEN
    RAISE EXCEPTION '#56 cascade setup FAIL: post not nested_object';
  END IF;
END $$;

CREATE FUNCTION _rc() RETURNS bigint LANGUAGE sql AS
  $$ SELECT (pg_tviews_queue_stats()->>'view_recomputes')::bigint $$;
CREATE FUNCTION _applied() RETURNS bigint LANGUAGE sql AS
  $$ SELECT (pg_tviews_queue_stats()->>'direct_patches_applied')::bigint $$;

-- ── Cycle 2: one-level cascade (user → post.author), zero recomputes ──────────
DO $$
DECLARE rc0 bigint; ap0 bigint;
BEGIN
  rc0 := _rc(); ap0 := _applied();
  UPDATE tb_user SET name = 'Bob' WHERE pk_user = 1;

  IF _rc() <> rc0 THEN
    RAISE EXCEPTION '#56 cascade FAIL: % view recompute(s) for a nested cascade', _rc() - rc0;
  END IF;
  -- user (1) + posts (3) + feeds (3) all patched directly.
  IF _applied() - ap0 <> 7 THEN
    RAISE EXCEPTION '#56 cascade FAIL: applied Δ% (expected 7)', _applied() - ap0;
  END IF;

  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'name' = 'Bob') <> 3 THEN
    RAISE EXCEPTION '#56 cascade FAIL: not all posts got author.name=Bob';
  END IF;
  -- Sibling keys of author, top-level title, and the comments array are untouched.
  IF (SELECT data->'author'->>'bio' FROM tv_post WHERE pk_post = 1) <> 'author-bio' THEN
    RAISE EXCEPTION '#56 cascade FAIL: author.bio sibling clobbered';
  END IF;
  IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) <> 'P1' THEN
    RAISE EXCEPTION '#56 cascade FAIL: title clobbered';
  END IF;
  IF (SELECT jsonb_array_length(data->'comments') FROM tv_post WHERE pk_post = 1) <> 2 THEN
    RAISE EXCEPTION '#56 cascade FAIL: comments array disturbed by author patch';
  END IF;
  -- Two-level: feed.post.author reflects the change.
  IF (SELECT count(*) FROM tv_feed WHERE data->'post'->'author'->>'name' = 'Bob') <> 3 THEN
    RAISE EXCEPTION '#56 cascade FAIL: feed.post.author.name not propagated (two-level)';
  END IF;
END $$;

-- ── Byte-identity: fast-path result == forced full recompute ─────────────────
DO $$
DECLARE fast_post jsonb; fast_feed jsonb; rec_post jsonb; rec_feed jsonb;
BEGIN
  UPDATE tb_user SET name = 'Carol', bio = 'new-bio' WHERE pk_user = 1;  -- fast path
  SELECT jsonb_agg(data ORDER BY pk_post) INTO fast_post FROM tv_post;
  SELECT jsonb_agg(data ORDER BY pk_feed) INTO fast_feed FROM tv_feed;

  PERFORM pg_tviews_refresh('user');
  PERFORM pg_tviews_refresh('post');
  PERFORM pg_tviews_refresh('feed');
  SELECT jsonb_agg(data ORDER BY pk_post) INTO rec_post FROM tv_post;
  SELECT jsonb_agg(data ORDER BY pk_feed) INTO rec_feed FROM tv_feed;

  IF fast_post <> rec_post THEN
    RAISE EXCEPTION '#56 cascade FAIL: tv_post fast-path not byte-identical to recompute';
  END IF;
  IF fast_feed <> rec_feed THEN
    RAISE EXCEPTION '#56 cascade FAIL: tv_feed fast-path not byte-identical to recompute';
  END IF;
END $$;

-- ── GUC-off differential: same mutation, GUC toggled, identical result ────────
DO $$
DECLARE on_data jsonb; off_data jsonb;
BEGIN
  SET pg_tviews.direct_patch_enabled = on;
  UPDATE tb_user SET name = 'Dave' WHERE pk_user = 1;
  SELECT jsonb_agg(data ORDER BY pk_post) INTO on_data FROM tv_post;

  SET pg_tviews.direct_patch_enabled = off;
  UPDATE tb_user SET name = 'Dave' WHERE pk_user = 1;   -- no-op value, forces recompute
  SELECT jsonb_agg(data ORDER BY pk_post) INTO off_data FROM tv_post;
  SET pg_tviews.direct_patch_enabled = on;

  IF on_data <> off_data THEN
    RAISE EXCEPTION '#56 cascade FAIL: GUC on/off differ for the cascade';
  END IF;
END $$;

-- ── Cycle 4: child fallback → parent recompute (poison propagation) ──────────
-- The CHILD (direct-entity) tview row is missing, so its own patch RETURNs
-- nothing ⇒ the child recomputes. A recomputed child cannot yield a trustworthy
-- derived patch, so its parents must recompute too (poison), not patch. Final
-- state is correct everywhere.
DO $$
DECLARE rc0 bigint; fb0 bigint;
BEGIN
  DELETE FROM tv_user WHERE pk_user = 1;   -- child row missing
  rc0 := _rc(); fb0 := (pg_tviews_queue_stats()->>'direct_patch_fallbacks')::bigint;
  UPDATE tb_user SET name = 'Eve' WHERE pk_user = 1;

  -- Child fell back at least once, and recompute(s) ran for the child + parents.
  IF (pg_tviews_queue_stats()->>'direct_patch_fallbacks')::bigint - fb0 < 1 THEN
    RAISE EXCEPTION '#56 cascade FAIL: missing child row did not fall back';
  END IF;
  IF _rc() = rc0 THEN
    RAISE EXCEPTION '#56 cascade FAIL: child fallback did not trigger recompute';
  END IF;
  -- The child row was re-materialised and all its posts reflect the change.
  IF (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) <> 'Eve' THEN
    RAISE EXCEPTION '#56 cascade FAIL: recomputed child row wrong/missing';
  END IF;
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'name' = 'Eve') <> 3 THEN
    RAISE EXCEPTION '#56 cascade FAIL: parents of a fallen-back child not refreshed';
  END IF;
  IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 2) <> 'P2' THEN
    RAISE EXCEPTION '#56 cascade FAIL: recomputed post lost its title';
  END IF;
END $$;

SELECT 'issue #56 cascade: PASS' AS result;

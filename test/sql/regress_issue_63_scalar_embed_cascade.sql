-- Regression test for issue #63: a base table that is BOTH a TVIEW's own source
-- AND a base-table (scalar) dependency of other TVIEWs must cascade to those parents.
--
-- `tb_user` backs `tv_user` directly, and `tv_post`/`tv_comment` embed the author
-- inline as a jsonb object built from base `tb_user` columns via a direct JOIN.
-- pg_tviews classifies that a `scalar` dependency (the parent references the base
-- TABLE's columns, not the `v_user.data` nested_object entity), so commit-time
-- entity propagation does NOT reach the parents — their refresh relies entirely on
-- the base-table cascade paths walked by `enqueue_cascade_parents`.
--
-- A row trigger that returns early for a direct TVIEW source (tb_user → user)
-- never walks those paths, so `tv_post.author` / `tv_comment.author` go STALE on a
-- `tb_user` UPDATE — a silent data-divergence bug. This is the benchmark's
-- full-embed read model.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_scalar_embed_cascade.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_comment CASCADE; DROP VIEW IF EXISTS v_comment CASCADE;
DROP TABLE IF EXISTS tv_post CASCADE;    DROP VIEW IF EXISTS v_post CASCADE;
DROP TABLE IF EXISTS tv_user CASCADE;    DROP VIEW IF EXISTS v_user CASCADE;
DROP TABLE IF EXISTS tb_comment CASCADE;
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
CREATE TABLE tb_comment (
    pk_comment INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author INTEGER REFERENCES tb_user(pk_user),
    fk_post INTEGER REFERENCES tb_post(pk_post), body TEXT
);

INSERT INTO tb_user (username, full_name, bio)
    VALUES ('alice', 'Alice A', 'a-bio'), ('zoe', 'Zoe Z', 'z-bio');
INSERT INTO tb_post (fk_author, title) VALUES (1, 'P1'), (1, 'P2'), (1, 'P3');
INSERT INTO tb_comment (fk_author, fk_post, body) VALUES (1, 1, 'c1'), (1, 2, 'c2');

-- Direct entity on tb_user.
SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id,
           jsonb_build_object('username', username, 'full_name', full_name, 'bio', bio) AS data
    FROM tb_user
$TVIEW$);

-- Post embeds the author inline, built from base tb_user columns via a direct JOIN.
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT p.pk_post, p.id, p.fk_author, u.id AS author_id,
           jsonb_build_object('title', p.title,
               'author', jsonb_build_object(
                   'username', u.username, 'full_name', u.full_name, 'bio', u.bio)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author
$TVIEW$);

-- Comment embeds its own author (from base tb_user) too.
SELECT pg_tviews_create('tv_comment', $TVIEW$
    SELECT c.pk_comment, c.id, c.fk_author, c.fk_post, u.id AS author_id,
           jsonb_build_object('body', c.body,
               'author', jsonb_build_object(
                   'username', u.username, 'full_name', u.full_name, 'bio', u.bio)) AS data
    FROM tb_comment c JOIN tb_user u ON u.pk_user = c.fk_author
$TVIEW$);

-- Precondition: the author embed is a scalar (base-table) dependency, not nested_object.
DO $$ BEGIN
  IF (SELECT dependency_types @> ARRAY['nested_object']::text[]
      FROM pg_tview_meta WHERE entity = 'post') THEN
    RAISE EXCEPTION 'setup FAIL: post classified nested_object (expected scalar/base-table embed)';
  END IF;
END $$;

-- Baseline: every post/comment shows author.username = alice.
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'username' = 'alice') <> 3 THEN
    RAISE EXCEPTION 'setup FAIL: initial author.username not "alice" on all 3 posts';
  END IF;
  IF (SELECT count(*) FROM tv_comment WHERE data->'author'->>'username' = 'alice') <> 2 THEN
    RAISE EXCEPTION 'setup FAIL: initial author.username not "alice" on all 2 comments';
  END IF;
END $$;

-- ── The fix: updating tb_user refreshes BOTH tv_user AND the embedded author in
--    every parent (the base-table cascade paths must be walked). ───────────────
UPDATE tb_user SET username = 'alice2', bio = 'a-bio-2' WHERE pk_user = 1;

DO $$ BEGIN
  -- Direct entity refreshed.
  IF (SELECT data->>'username' FROM tv_user WHERE pk_user = 1) <> 'alice2' THEN
    RAISE EXCEPTION 'FAIL: tv_user direct refresh missing (got %)',
      (SELECT data->>'username' FROM tv_user WHERE pk_user = 1);
  END IF;
  -- Embedded author in the posts refreshed — this is the fall-through fix.
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'username' = 'alice2') <> 3 THEN
    RAISE EXCEPTION 'FAIL: tb_user edit did not cascade to tv_post.author.username (stale embed): % of 3',
      (SELECT count(*) FROM tv_post WHERE data->'author'->>'username' = 'alice2');
  END IF;
  IF (SELECT count(*) FROM tv_post WHERE data->'author'->>'bio' = 'a-bio-2') <> 3 THEN
    RAISE EXCEPTION 'FAIL: tv_post.author.bio stale after tb_user edit';
  END IF;
  -- Comments too.
  IF (SELECT count(*) FROM tv_comment WHERE data->'author'->>'username' = 'alice2') <> 2 THEN
    RAISE EXCEPTION 'FAIL: tb_user edit did not cascade to tv_comment.author.username';
  END IF;
  -- Unrelated top-level fields untouched.
  IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) <> 'P1' THEN
    RAISE EXCEPTION 'FAIL: tv_post.title clobbered by cascade';
  END IF;
END $$;

SELECT 'scalar-embed cascade: PASS' AS result;

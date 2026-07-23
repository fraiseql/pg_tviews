-- Hardening: column-aware refresh with a self-join / two-level embed — the
-- benchmark's tv_comment, which JOINs tb_user TWICE (once as the comment author,
-- once as the post author two levels down). column-level pg_depend records the
-- UNION of columns referenced across both aliases, so a change to any referenced
-- tb_user column must cascade to BOTH roles, while an unreferenced column skips.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_column_aware_selfjoin.sql

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
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE, username TEXT, bio TEXT
);
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author INTEGER REFERENCES tb_user, title TEXT
);
CREATE TABLE tb_comment (
    pk_comment INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author INTEGER REFERENCES tb_user, fk_post INTEGER REFERENCES tb_post, body TEXT
);

-- alice(1) authors the post; bob(2) and alice(1) each comment on it, so alice
-- appears BOTH as a post author (2 levels down) and as a comment author.
INSERT INTO tb_user (username, bio) VALUES ('alice', 'a-bio'), ('bob', 'b-bio');
INSERT INTO tb_post (fk_author, title) VALUES (1, 'T1');
INSERT INTO tb_comment (fk_author, fk_post, body) VALUES (2, 1, 'c-bob'), (1, 1, 'c-alice');

SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('username', username, 'bio', bio) AS data
    FROM tb_user
$TVIEW$);
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT p.pk_post, p.id, p.fk_author, u.id AS author_id,
           jsonb_build_object('title', p.title,
               'author', jsonb_build_object('username', u.username)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author
$TVIEW$);
-- tb_user joined twice: u = comment author, pu = post author (two levels down).
-- Neither role embeds bio.
SELECT pg_tviews_create('tv_comment', $TVIEW$
    SELECT c.pk_comment, c.id, c.fk_author, c.fk_post, u.id AS author_id, p.id AS post_id,
           jsonb_build_object('body', c.body,
               'author', jsonb_build_object('username', u.username),
               'post',   jsonb_build_object('title', p.title,
                           'author', jsonb_build_object('username', pu.username))) AS data
    FROM tb_comment c
    JOIN tb_user u  ON u.pk_user  = c.fk_author
    JOIN tb_post p  ON p.pk_post  = c.fk_post
    JOIN tb_user pu ON pu.pk_user = p.fk_author
$TVIEW$);

CREATE FUNCTION _rc() RETURNS bigint LANGUAGE sql AS
  $$ SELECT (pg_tviews_queue_stats()->>'view_recomputes')::bigint $$;
CREATE FUNCTION _c_author(pk int)  RETURNS text LANGUAGE sql AS
  $$ SELECT data->'author'->>'username' FROM tv_comment WHERE pk_comment = pk $$;
CREATE FUNCTION _c_pauthor(pk int) RETURNS text LANGUAGE sql AS
  $$ SELECT data->'post'->'author'->>'username' FROM tv_comment WHERE pk_comment = pk $$;

-- Baseline: comment1 = {author bob, post.author alice}; comment2 = {author alice, post.author alice}.
DO $$ BEGIN
  IF _c_author(1) <> 'bob'   OR _c_pauthor(1) <> 'alice' THEN RAISE EXCEPTION 'setup FAIL c1'; END IF;
  IF _c_author(2) <> 'alice' OR _c_pauthor(2) <> 'alice' THEN RAISE EXCEPTION 'setup FAIL c2'; END IF;
END $$;

-- ── Change the comment author only (bob) → comment1.author, not post.author ──
DO $$ BEGIN
  UPDATE tb_user SET username = 'bob2' WHERE pk_user = 2;
  IF _c_author(1) <> 'bob2' THEN
    RAISE EXCEPTION 'selfjoin FAIL: comment.author not refreshed for bob (got %)', _c_author(1);
  END IF;
  IF _c_pauthor(1) <> 'alice' THEN
    RAISE EXCEPTION 'selfjoin FAIL: comment.post.author wrongly changed by a comment-author edit';
  END IF;
END $$;

-- ── Change alice → hits post.author (2 levels) in c1 AND both roles in c2 ────
DO $$ BEGIN
  UPDATE tb_user SET username = 'alice2' WHERE pk_user = 1;
  IF _c_pauthor(1) <> 'alice2' THEN
    RAISE EXCEPTION 'selfjoin FAIL: two-level post.author not refreshed in comment1 (got %)', _c_pauthor(1);
  END IF;
  IF _c_author(2) <> 'alice2' OR _c_pauthor(2) <> 'alice2' THEN
    RAISE EXCEPTION 'selfjoin FAIL: dual-role user not refreshed in comment2 (author=%, post.author=%)',
      _c_author(2), _c_pauthor(2);
  END IF;
  IF _c_author(1) <> 'bob2' THEN
    RAISE EXCEPTION 'selfjoin FAIL: comment1.author (bob2) disturbed by alice edit';
  END IF;
END $$;

-- ── Change alice.bio (unreferenced by tv_post/tv_comment) → cascades SKIPPED ─
DO $$
DECLARE rc0 bigint;
BEGIN
  rc0 := _rc();
  UPDATE tb_user SET bio = 'a-bio-CHANGED' WHERE pk_user = 1;
  IF _rc() <> rc0 THEN
    RAISE EXCEPTION 'selfjoin FAIL: unreferenced bio triggered % recompute(s)', _rc() - rc0;
  END IF;
  IF (SELECT data->>'bio' FROM tv_user WHERE pk_user = 1) <> 'a-bio-CHANGED' THEN
    RAISE EXCEPTION 'selfjoin FAIL: tv_user.bio not refreshed (direct entity)';
  END IF;
  -- The comment embeds remain correct (never embedded bio).
  IF _c_pauthor(1) <> 'alice2' OR _c_author(2) <> 'alice2' THEN
    RAISE EXCEPTION 'selfjoin FAIL: comment embeds disturbed by a skipped bio edit';
  END IF;
END $$;

-- ── Byte-identity ────────────────────────────────────────────────────────────
DO $$
DECLARE fast jsonb; rec jsonb;
BEGIN
  SELECT jsonb_agg(data ORDER BY pk_comment) INTO fast FROM tv_comment;
  PERFORM pg_tviews_refresh('comment');
  SELECT jsonb_agg(data ORDER BY pk_comment) INTO rec FROM tv_comment;
  IF fast <> rec THEN
    RAISE EXCEPTION 'selfjoin FAIL: tv_comment not byte-identical to a forced recompute';
  END IF;
END $$;

SELECT 'column-aware selfjoin: PASS' AS result;

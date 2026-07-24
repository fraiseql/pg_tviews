-- Regression test: multi-hop column-aware propagation prune.
--
-- Flush-time ENTITY propagation (src/propagate.rs, driven by EntityDepGraph.parents)
-- fired for EVERY change to a child entity, unconditionally recomputing every parent
-- that embeds it. For a scalar embed reading only the child's own columns (a comment
-- embedding the post's {title}) that recompute is pure waste: the column can change
-- only via the child's base table, which is already covered by the column-aware
-- tb_<child> cascade path. Worse, it fired even when the child was recomputed for an
-- UNRELATED deeper embed — a post refreshed because its author's bio changed dragged
-- every comment on that post through a data-preserving recompute.
--
-- On a three-level chain user -> post -> comment this proves:
--   * updateUser(bio) refreshes tv_post (which embeds the whole author document) but
--     does NOT process the comment entity at all (propagation edge pruned) and leaves
--     tv_comment BYTE-IDENTICAL — the comment embeds only post.title, so there is no
--     staleness;
--   * changing post.title DOES refresh tv_comment (via the tb_post cascade path that
--     covers the pruned edge) — the prune introduces no staleness when the embedded
--     column actually changes;
--   * a newly inserted comment embeds the current post title;
--   * the fast path stays byte-identical to a forced full recompute.
--
-- The complementary graph-shape property — nested_object/array embeds, and scalar
-- embeds that follow a child FK, are KEPT — is covered by the #[pg_test]
-- test_scalar_embed_propagation_is_fk_aware in src/queue/graph.rs.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_multihop_propagation_prune.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- Per-entity REFRESH events land in pg_tview_audit_log; used below to prove the
-- comment entity is (or is not) processed by a given mutation.
SET pg_tviews.audit_enabled = on;

DROP TABLE IF EXISTS tv_comment CASCADE; DROP VIEW IF EXISTS v_comment CASCADE;
DROP TABLE IF EXISTS tv_post CASCADE;    DROP VIEW IF EXISTS v_post CASCADE;
DROP TABLE IF EXISTS tv_user CASCADE;    DROP VIEW IF EXISTS v_user CASCADE;
DROP TABLE IF EXISTS tb_comment CASCADE;
DROP TABLE IF EXISTS tb_post CASCADE;
DROP TABLE IF EXISTS tb_user CASCADE;

CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    username TEXT, bio TEXT
);
-- The FK is named fk_user (matching the referenced entity): flush-time entity
-- propagation keys parents by fk_<entity>, so this is what makes user -> post a
-- propagation edge at all.
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

INSERT INTO tb_user (username, bio) VALUES ('alice', 'a-bio');
INSERT INTO tb_post (fk_user, title) VALUES (1, 'P1');
INSERT INTO tb_comment (fk_post, body)
    VALUES (1, 'C1'), (1, 'C2'), (1, 'C3'), (1, 'C4'), (1, 'C5');

-- tv_user: embeds bio (its own base column).
SELECT pg_tviews_create('tv_user', $TVIEW$
    SELECT pk_user, id, jsonb_build_object('username', username, 'bio', bio) AS data
    FROM tb_user
$TVIEW$);

-- tv_post: embeds the whole COMPUTED user document (nested_object dependency) → the
-- user -> post edge is a genuine propagation edge and must be kept.
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', v_user.data) AS data
    FROM tb_post p LEFT JOIN v_user ON v_user.pk_user = p.fk_user
$TVIEW$);

-- tv_comment: scalar embed of ONLY the post's own title, read from tb_post columns
-- via a direct JOIN → the post -> comment propagation edge is redundant with the
-- column-aware tb_post cascade path and is pruned.
SELECT pg_tviews_create('tv_comment', $TVIEW$
    SELECT c.pk_comment, c.id, c.fk_post,
           jsonb_build_object('body', c.body,
               'post', jsonb_build_object('title', p.title)) AS data
    FROM tb_comment c JOIN tb_post p ON p.pk_post = c.fk_post
$TVIEW$);

CREATE FUNCTION _refreshed(ent text) RETURNS bigint LANGUAGE sql AS
  $$ SELECT count(*) FROM pg_tview_audit_log WHERE entity = ent AND operation = 'REFRESH' $$;

-- Sanity: comments embed the initial post title.
DO $$
BEGIN
  IF (SELECT count(*) FROM tv_comment WHERE data->'post'->>'title' = 'P1') <> 5 THEN
    RAISE EXCEPTION 'setup FAIL: comments do not embed the initial post title';
  END IF;
END $$;

-- ── Case 1: updateUser(bio) refreshes the post but PRUNES the comments ────────────
-- bio is embedded by tv_post (nested author document) but NOT by tv_comment. The
-- comment entity must not be processed at all, and its rows must stay byte-identical.
DO $$
DECLARE c0 bigint; before jsonb; after jsonb;
BEGIN
  SELECT jsonb_agg(data ORDER BY pk_comment) INTO before FROM tv_comment;
  c0 := _refreshed('comment');

  UPDATE tb_user SET bio = 'a-bio-CHANGED' WHERE pk_user = 1;

  -- Prune: the comment entity was never processed (no REFRESH audit event).
  IF _refreshed('comment') <> c0 THEN
    RAISE EXCEPTION 'prune FAIL: bio update processed the comment entity % time(s) (expected 0)',
      _refreshed('comment') - c0;
  END IF;
  -- No staleness: comment documents are byte-identical (they never embedded bio).
  SELECT jsonb_agg(data ORDER BY pk_comment) INTO after FROM tv_comment;
  IF before IS DISTINCT FROM after THEN
    RAISE EXCEPTION 'prune FAIL: tv_comment changed on an unrelated bio update';
  END IF;
  -- The kept nested edge still works: tv_post reflects the new bio.
  IF (SELECT data->'author'->>'bio' FROM tv_post WHERE pk_post = 1) <> 'a-bio-CHANGED' THEN
    RAISE EXCEPTION 'propagation FAIL: tv_post.author.bio not refreshed (user->post edge lost)';
  END IF;
END $$;

-- ── Case 2: changing post.title refreshes the comments via the tb_post cascade path ─
-- The pruned propagation edge must be fully covered by the column-aware base path.
DO $$
DECLARE c0 bigint;
BEGIN
  c0 := _refreshed('comment');
  UPDATE tb_post SET title = 'P1-RENAMED' WHERE pk_post = 1;

  IF _refreshed('comment') = c0 THEN
    RAISE EXCEPTION 'coverage FAIL: post.title change did not refresh any comment';
  END IF;
  IF (SELECT count(*) FROM tv_comment WHERE data->'post'->>'title' = 'P1-RENAMED') <> 5 THEN
    RAISE EXCEPTION 'coverage FAIL: only % of 5 comments reflect the new post title',
      (SELECT count(*) FROM tv_comment WHERE data->'post'->>'title' = 'P1-RENAMED');
  END IF;
END $$;

-- ── Case 3: a newly inserted comment embeds the current post title ────────────────
DO $$
BEGIN
  INSERT INTO tb_comment (fk_post, body) VALUES (1, 'C6');
  IF (SELECT data->'post'->>'title' FROM tv_comment WHERE data->>'body' = 'C6') <> 'P1-RENAMED' THEN
    RAISE EXCEPTION 'insert FAIL: new comment did not embed the current post title';
  END IF;
END $$;

-- ── Case 4: byte-identity — fast path == forced full recompute ────────────────────
DO $$
DECLARE fast jsonb; rec jsonb;
BEGIN
  SELECT jsonb_agg(data ORDER BY pk_comment) INTO fast FROM tv_comment;
  PERFORM pg_tviews_refresh('comment');
  SELECT jsonb_agg(data ORDER BY pk_comment) INTO rec FROM tv_comment;
  IF fast IS DISTINCT FROM rec THEN
    RAISE EXCEPTION 'byte-identity FAIL: tv_comment not identical to a forced recompute';
  END IF;
END $$;

SELECT 'multi-hop propagation prune: PASS' AS result;

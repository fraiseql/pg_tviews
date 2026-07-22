-- Regression test for issue #50:
--   "Smart-patch path calls jsonb_smart_patch_array(jsonb,jsonb,text[],text),
--    which jsonb_delta 0.1.0 does not export — the test stub masked the mismatch."
--
-- Against the REAL jsonb_delta, an array-dependency refresh used to error with
-- `function jsonb_smart_patch_array(jsonb, jsonb, text[], text) does not exist`.
-- pg_tviews now recomputes the whole row for a tview with an array dependency
-- (jsonb_delta 0.1.0 cannot surgically sync a whole array, and a path-level
-- replace would miss the entity's own-column changes), so array element
-- update/insert/delete AND own-column changes all refresh correctly.
--
-- Part B exercises the availability-latch invalidation (Cycle 3): DROP/CREATE
-- EXTENSION jsonb_delta within one backend must be observed immediately.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_50_array_signature.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_post CASCADE;
DROP VIEW  IF EXISTS v_post CASCADE;
DROP TABLE IF EXISTS tb_comment CASCADE;
DROP TABLE IF EXISTS tb_post CASCADE;

CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title   TEXT
);
CREATE TABLE tb_comment (
    pk_comment INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_post    INTEGER NOT NULL REFERENCES tb_post(pk_post),
    body       TEXT
);

INSERT INTO tb_post (title) VALUES ('Hello');
INSERT INTO tb_comment (fk_post, body) VALUES (1, 'first'), (1, 'second');

-- tv_post aggregates its comments into a JSONB array (an Array dependency).
SELECT pg_tviews_create('tv_post', $TVIEW$
    SELECT p.pk_post AS pk_post, p.id AS id,
           jsonb_build_object(
               'title', p.title,
               'comments', COALESCE(
                   jsonb_agg(jsonb_build_object('id', c.pk_comment, 'body', c.body)
                             ORDER BY c.pk_comment) FILTER (WHERE c.pk_comment IS NOT NULL),
                   '[]'::jsonb)
           ) AS data
    FROM tb_post p
    LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
    GROUP BY p.pk_post, p.id, p.title
$TVIEW$);

DO $$ BEGIN
  IF (SELECT jsonb_array_length(data->'comments') FROM tv_post WHERE pk_post = 1) <> 2 THEN
    RAISE EXCEPTION '#50 setup FAIL: expected 2 comments initially';
  END IF;
END $$;

-- UPDATE a comment — previously errored ("jsonb_smart_patch_array ... does not exist").
UPDATE tb_comment SET body = 'first-updated' WHERE pk_comment = 1;
DO $$ BEGIN
  IF (SELECT data->'comments'->0->>'body' FROM tv_post WHERE pk_post = 1) <> 'first-updated' THEN
    RAISE EXCEPTION '#50 FAIL: array element UPDATE did not refresh (got %)',
      (SELECT data->'comments'->0->>'body' FROM tv_post WHERE pk_post = 1);
  END IF;
END $$;

-- INSERT a new comment — must appear in the array.
INSERT INTO tb_comment (fk_post, body) VALUES (1, 'third');
DO $$ BEGIN
  IF (SELECT jsonb_array_length(data->'comments') FROM tv_post WHERE pk_post = 1) <> 3 THEN
    RAISE EXCEPTION '#50 FAIL: array element INSERT not reflected (len %)',
      (SELECT jsonb_array_length(data->'comments') FROM tv_post WHERE pk_post = 1);
  END IF;
END $$;

-- DELETE a comment — must disappear from the array.
DELETE FROM tb_comment WHERE body = 'second';
DO $$ BEGIN
  IF (SELECT jsonb_array_length(data->'comments') FROM tv_post WHERE pk_post = 1) <> 2 THEN
    RAISE EXCEPTION '#50 FAIL: array element DELETE not reflected (len %)',
      (SELECT jsonb_array_length(data->'comments') FROM tv_post WHERE pk_post = 1);
  END IF;
END $$;

-- Own-column change on the entity's own base table must also refresh (the earlier
-- path-level array patch missed this; full replacement covers it).
UPDATE tb_post SET title = 'Hello-Updated' WHERE pk_post = 1;
DO $$ BEGIN
  IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) <> 'Hello-Updated' THEN
    RAISE EXCEPTION '#50 FAIL: own-column UPDATE on array-dep tview not refreshed (got %)',
      (SELECT data->>'title' FROM tv_post WHERE pk_post = 1);
  END IF;
END $$;

-- Part B — availability-latch invalidation on extension DDL (Cycle 3).
DO $$ BEGIN
  IF NOT pg_tviews_check_jsonb_delta() THEN
    RAISE EXCEPTION '#50 FAIL: jsonb_delta should be reported available';
  END IF;
END $$;

DROP EXTENSION jsonb_delta;   -- ProcessUtility hook must invalidate the latch
DO $$ BEGIN
  IF pg_tviews_check_jsonb_delta() THEN
    RAISE EXCEPTION '#50 FAIL: availability latch stale-true after DROP EXTENSION jsonb_delta';
  END IF;
END $$;

CREATE EXTENSION jsonb_delta; -- hook must invalidate again
DO $$ BEGIN
  IF NOT pg_tviews_check_jsonb_delta() THEN
    RAISE EXCEPTION '#50 FAIL: availability latch stale-false after re-CREATE EXTENSION jsonb_delta';
  END IF;
END $$;

\echo '#50 PASS: array-dep refresh correct (element + own-column); latch re-checked on extension DDL'

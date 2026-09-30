-- Regression test for issue #76: pg_tviews_flush_and_report(), the read-model rows a
-- transaction changed, in the GraphQL Cascade shape.
--
-- The AFTER STATEMENT flush trigger flushes after every statement, so by the end of
-- a mutation function the queue is long empty. The report must come from what the
-- refreshes actually wrote during the transaction: rows changed by cascades
-- included, no-op refreshes and rolled-back savepoints excluded.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_76_flush_and_report.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT
);
CREATE TABLE tb_blog_post (
    pk_blog_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id           UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user      BIGINT NOT NULL REFERENCES tb_user(pk_user),
    title        TEXT
);
INSERT INTO tb_user (name) VALUES ('ann'), ('bob');
INSERT INTO tb_blog_post (fk_user, title) VALUES (1, 'p1'), (1, 'p2'), (2, 'p3');

CREATE TABLE tv_user AS
SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user;
CREATE TABLE tv_blog_post AS
SELECT p.pk_blog_post, p.id, p.fk_user,
       jsonb_build_object('title', p.title, 'author', u.name) AS data
FROM tb_blog_post p JOIN tb_user u ON u.pk_user = p.fk_user;

CREATE FUNCTION update_post(p BIGINT, t TEXT, author TEXT) RETURNS JSONB LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_blog_post SET title = t WHERE pk_blog_post = p;
    UPDATE tb_user SET name = author WHERE pk_user = 1;
    RETURN pg_tviews_flush_and_report();
END $$;

-- (typename, id-of, operation) triples of a report's updated list, sorted.
CREATE FUNCTION updated_of(r JSONB) RETURNS TEXT[] LANGUAGE sql AS $$
    SELECT coalesce(array_agg(x ORDER BY x), '{}')
    FROM (SELECT format('%s:%s:%s', e->>'__typename', e->>'id', e->>'operation') AS x
          FROM jsonb_array_elements(r->'updated') e) s
$$;
CREATE FUNCTION expect(entries TEXT[]) RETURNS TEXT[] LANGUAGE sql AS $$
    SELECT array_agg(x ORDER BY x) FROM unnest(entries) x
$$;

-- ========================================================================
-- Cycle 1: statement-level flushes inside a mutation are all reported
-- ========================================================================
CREATE TEMP TABLE r AS SELECT update_post(1, 'new', 'ann2') AS report;

DO $$
DECLARE
    rep JSONB := (SELECT report FROM r);
    u1 TEXT := (SELECT id FROM tb_user WHERE pk_user = 1);
    p1 TEXT := (SELECT id FROM tb_blog_post WHERE pk_blog_post = 1);
    p2 TEXT := (SELECT id FROM tb_blog_post WHERE pk_blog_post = 2);
BEGIN
  IF updated_of(rep) IS DISTINCT FROM expect(ARRAY[
       'BlogPost:' || p1 || ':UPDATED', 'BlogPost:' || p2 || ':UPDATED', 'User:' || u1 || ':UPDATED']) THEN
    RAISE EXCEPTION '#76 FAIL: updated = %', updated_of(rep);
  END IF;
  IF (SELECT e->'data'->>'title' FROM jsonb_array_elements(rep->'updated') e
      WHERE e->>'id' = p1) IS DISTINCT FROM 'new' THEN
    RAISE EXCEPTION '#76 FAIL: reported data is not the fresh row: %', rep;
  END IF;
  IF jsonb_array_length(rep->'deleted') <> 0 OR (rep->>'truncated')::bool THEN
    RAISE EXCEPTION '#76 FAIL: unexpected deleted/truncated: %', rep;
  END IF;
END $$;

-- ========================================================================
-- Cycle 2: a no-op mutation reports nothing
-- ========================================================================
DO $$
DECLARE rep JSONB;
BEGIN
  UPDATE tb_user SET name = name WHERE pk_user = 1;
  rep := pg_tviews_flush_and_report();
  IF jsonb_array_length(rep->'updated') <> 0 THEN
    RAISE EXCEPTION '#76 FAIL: no-op update reported %', rep;
  END IF;
END $$;

-- ========================================================================
-- Cycle 3: inserts are CREATED, deletes carry the id, include_data => false
-- ========================================================================
DO $$
DECLARE
    rep JSONB;
    gone TEXT := (SELECT id FROM tb_blog_post WHERE pk_blog_post = 3);
    fresh TEXT;
BEGIN
  DELETE FROM tb_blog_post WHERE pk_blog_post = 3;
  INSERT INTO tb_blog_post (fk_user, title) VALUES (2, 'p4') RETURNING id INTO fresh;
  rep := pg_tviews_flush_and_report(include_data => false);
  IF updated_of(rep) IS DISTINCT FROM ARRAY['BlogPost:' || fresh || ':CREATED'] THEN
    RAISE EXCEPTION '#76 FAIL: insert reported as %', updated_of(rep);
  END IF;
  IF rep->'deleted' IS DISTINCT FROM jsonb_build_array(
       jsonb_build_object('__typename', 'BlogPost', 'id', gone)) THEN
    RAISE EXCEPTION '#76 FAIL: deleted = %', rep->'deleted';
  END IF;
  IF EXISTS (SELECT 1 FROM jsonb_array_elements(rep->'updated') e WHERE e ? 'data') THEN
    RAISE EXCEPTION '#76 FAIL: include_data => false still returned data';
  END IF;
END $$;

-- ========================================================================
-- Cycle 4: a rolled-back savepoint is not reported
-- ========================================================================
BEGIN;
SAVEPOINT s;
UPDATE tb_user SET name = 'rolled back' WHERE pk_user = 2;
ROLLBACK TO SAVEPOINT s;
UPDATE tb_blog_post SET title = 'kept' WHERE pk_blog_post = 2;
SELECT pg_tviews_flush_and_report() AS report \gset
COMMIT;
SELECT updated_of(:'report'::jsonb) = ARRAY['BlogPost:' || (SELECT id FROM tb_blog_post WHERE pk_blog_post = 2) || ':UPDATED']
       AS savepoint_ok \gset
\if :savepoint_ok
\else
  DO $$ BEGIN RAISE EXCEPTION '#76 FAIL: savepoint rollback leaked into the report'; END $$;
\endif

-- ========================================================================
-- Cycle 5: max_entities truncates deterministically; typename override; reset
-- ========================================================================
SELECT pg_tviews_set_typename('blog_post', 'Article');
DO $$
DECLARE rep JSONB; again JSONB;
BEGIN
  UPDATE tb_user SET name = 'ann3' WHERE pk_user = 1;   -- User 1 + its 2 posts
  rep := pg_tviews_flush_and_report(max_entities => 1);
  IF jsonb_array_length(rep->'updated') <> 1 OR NOT (rep->>'truncated')::bool
     OR rep->'invalidated_types' IS DISTINCT FROM '["Article", "User"]'::jsonb
     OR rep->'updated'->0->>'__typename' <> 'Article'
     OR rep->'updated'->0->>'id' <> (SELECT id::text FROM tb_blog_post WHERE pk_blog_post = 1) THEN
    RAISE EXCEPTION '#76 FAIL: truncation/typename %', rep;
  END IF;
  again := pg_tviews_flush_and_report();
  IF jsonb_array_length(again->'updated') <> 0 THEN
    RAISE EXCEPTION '#76 FAIL: reset did not clear the journal: %', again;
  END IF;
END $$;

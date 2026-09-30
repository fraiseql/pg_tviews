-- Regression test for issue #57:
--   "sql_drop cleanup only covers the eponymous tview when a base table is
--    dropped."
--
-- The #53 handler matched a dropped tb_<entity> to the TVIEW named <entity>. A
-- TVIEW that reads the dropped table under another name (a join, a report over
-- someone else's table) lost its backing view to CASCADE but kept its tv_* table,
-- its pg_tview_meta row and its triggers on the surviving base tables.
--
-- Correct behaviour: whenever a TVIEW's backing view or table is dropped as a
-- dependent of something else (a base table, a helper view, a schema), the TVIEW
-- is deregistered with all its objects, including triggers on other tables.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_57_drop_cleanup.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_author (
    pk_author BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name      TEXT
);
CREATE TABLE tb_post (
    pk_post   BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author BIGINT,
    title     TEXT
);
INSERT INTO tb_author (name) VALUES ('ann');
INSERT INTO tb_post (fk_author, title) VALUES (1, 'p1');

-- ========================================================================
-- Cycle 1: dropping a joined (non-eponymous) base table deregisters the TVIEW
-- ========================================================================
CREATE TABLE tv_post AS
SELECT p.pk_post, p.id, p.fk_author,
       jsonb_build_object('title', p.title, 'author', a.name) AS data
FROM tb_post p JOIN tb_author a ON a.pk_author = p.fk_author;

DROP TABLE tb_author CASCADE;

DO $$ BEGIN
  IF to_regclass('tv_post') IS NOT NULL THEN
    RAISE EXCEPTION '#57 FAIL: tv_post orphaned after DROP TABLE tb_author CASCADE';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post') THEN
    RAISE EXCEPTION '#57 FAIL: stale pg_tview_meta row for post';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'tb_post'::regclass
               AND tgname LIKE 'trg_tview%') THEN
    RAISE EXCEPTION '#57 FAIL: triggers left on tb_post: %',
      (SELECT array_agg(tgname) FROM pg_trigger WHERE tgrelid = 'tb_post'::regclass
         AND tgname LIKE 'trg_tview%');
  END IF;
END $$;

-- The surviving base table still takes writes.
INSERT INTO tb_post (fk_author, title) VALUES (1, 'p2');

-- ========================================================================
-- Cycle 2: dropping a helper view the backing view reads
-- ========================================================================
CREATE VIEW v_post_title AS SELECT pk_post, id, title FROM tb_post;
CREATE TABLE tv_post AS
SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM v_post_title;

DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post') THEN
    RAISE EXCEPTION '#57 setup FAIL: tv_post over v_post_title not registered';
  END IF;
END $$;

DROP VIEW v_post_title CASCADE;

DO $$ BEGIN
  IF to_regclass('tv_post') IS NOT NULL
     OR EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post') THEN
    RAISE EXCEPTION '#57 FAIL: tv_post not deregistered after its helper view was dropped';
  END IF;
END $$;

-- ========================================================================
-- Cycle 3: DROP SCHEMA ... CASCADE deregisters the TVIEWs it contained
-- ========================================================================
CREATE SCHEMA app;
CREATE TABLE app.tb_tag (
    pk_tag BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id     UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    label  TEXT
);
CREATE TABLE app.tv_tag AS
SELECT pk_tag, id, jsonb_build_object('label', label) AS data FROM app.tb_tag;

DROP SCHEMA app CASCADE;

DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'tag') THEN
    RAISE EXCEPTION '#57 FAIL: stale pg_tview_meta row for tag after DROP SCHEMA app CASCADE';
  END IF;
END $$;

-- ========================================================================
-- Cycle 4: direct drops still work exactly once; unrelated drops are ignored
-- ========================================================================
CREATE TABLE tv_post AS
SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;
SELECT pg_tviews_drop('post');
DO $$ BEGIN
  IF to_regclass('tv_post') IS NOT NULL OR to_regclass('v_post') IS NOT NULL
     OR EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post')
     OR EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'tb_post'::regclass
                  AND tgname LIKE 'trg_tview%') THEN
    RAISE EXCEPTION '#57 FAIL: pg_tviews_drop(post) left objects behind';
  END IF;
END $$;

CREATE TABLE tv_post AS
SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;
DROP TABLE tv_post;
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post') THEN
    RAISE EXCEPTION '#57 FAIL: DROP TABLE tv_post left metadata';
  END IF;
END $$;

CREATE TABLE tv_post AS
SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;
CREATE VIEW v_unrelated AS SELECT 1 AS x;
DROP VIEW v_unrelated;
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post') THEN
    RAISE EXCEPTION '#57 FAIL: an unrelated DROP VIEW deregistered tv_post';
  END IF;
END $$;

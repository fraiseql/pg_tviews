-- Regression test for issue #81:
--   "pg_tview_meta.definition goes stale when a base-table column is renamed."
--
-- PostgreSQL rewrites the backing view v_<entity> to follow a column rename, but
-- the stored definition, and the metadata derived from it at creation, kept the
-- old name. Beyond the stale text, propagation broke silently: the column-aware
-- cascade skipped updates to a renamed joined column, and a renamed FK stopped
-- the cascade altogether.
--
-- Correct behaviour: after ALTER TABLE ... RENAME COLUMN, the definition is the
-- author's text with the renamed references rewritten (a bare select item keeps
-- its output name via AS), and the TVIEW keeps propagating.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_81_rename_column.sql

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
    fk_author BIGINT NOT NULL REFERENCES tb_author(pk_author),
    title     TEXT
);
INSERT INTO tb_author (name) VALUES ('ann');
INSERT INTO tb_post (fk_author, title) VALUES (1, 't1');

CREATE TABLE tv_post AS
SELECT p.pk_post, p.id, p.fk_author,
       jsonb_build_object('title', p.title, 'author', a.name) AS data
FROM tb_post p JOIN tb_author a ON a.pk_author = p.fk_author;

-- ========================================================================
-- Cycle 1: renaming an own column rewrites the definition; updates propagate
-- ========================================================================
ALTER TABLE tb_post RENAME COLUMN title TO headline;

DO $$ BEGIN
  IF (SELECT definition FROM pg_tview_meta WHERE entity = 'post') IS DISTINCT FROM
     E'SELECT p.pk_post, p.id, p.fk_author,\n       jsonb_build_object(''title'', p.headline, ''author'', a.name) AS data\nFROM tb_post p JOIN tb_author a ON a.pk_author = p.fk_author' THEN
    RAISE EXCEPTION '#81 FAIL: definition after renaming title: %',
      (SELECT definition FROM pg_tview_meta WHERE entity = 'post');
  END IF;
  IF NOT (SELECT 'headline' = ANY (direct_map_columns) FROM pg_tview_meta WHERE entity = 'post') THEN
    RAISE EXCEPTION '#81 FAIL: direct_map_columns still names the old column: %',
      (SELECT direct_map_columns FROM pg_tview_meta WHERE entity = 'post');
  END IF;
END $$;

UPDATE tb_post SET headline = 't2';
DO $$ BEGIN
  IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 't2' THEN
    RAISE EXCEPTION '#81 FAIL: update of the renamed own column did not propagate';
  END IF;
END $$;

-- ========================================================================
-- Cycle 2: renaming a joined column keeps the cascade (column-aware set)
-- ========================================================================
ALTER TABLE tb_author RENAME COLUMN name TO full_name;
UPDATE tb_author SET full_name = 'bob';
DO $$ BEGIN
  IF (SELECT data->>'author' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 'bob' THEN
    RAISE EXCEPTION '#81 FAIL: update of the renamed joined column did not cascade (got %)',
      (SELECT data->>'author' FROM tv_post WHERE pk_post = 1);
  END IF;
END $$;

-- ========================================================================
-- Cycle 3: renaming the FK keeps the output column name and the cascade
-- ========================================================================
ALTER TABLE tb_post RENAME COLUMN fk_author TO fk_writer;

DO $$ BEGIN
  IF (SELECT definition FROM pg_tview_meta WHERE entity = 'post') IS DISTINCT FROM
     E'SELECT p.pk_post, p.id, p.fk_writer AS fk_author,\n       jsonb_build_object(''title'', p.headline, ''author'', a.full_name) AS data\nFROM tb_post p JOIN tb_author a ON a.pk_author = p.fk_writer' THEN
    RAISE EXCEPTION '#81 FAIL: definition after renaming fk_author: %',
      (SELECT definition FROM pg_tview_meta WHERE entity = 'post');
  END IF;
END $$;

UPDATE tb_author SET full_name = 'cid';
INSERT INTO tb_post (fk_writer, headline) VALUES (1, 't3');
DO $$ BEGIN
  IF (SELECT data->>'author' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 'cid' THEN
    RAISE EXCEPTION '#81 FAIL: cascade stopped after renaming the FK (got %)',
      (SELECT data->>'author' FROM tv_post WHERE pk_post = 1);
  END IF;
  IF (SELECT fk_author FROM tv_post WHERE pk_post = 2) IS DISTINCT FROM 1 THEN
    RAISE EXCEPTION '#81 FAIL: insert after renaming the FK did not reach tv_post';
  END IF;
END $$;

-- ========================================================================
-- Cycle 4: a rename the text rewrite cannot follow falls back to the view text
-- ========================================================================
CREATE TABLE tb_tag (
    pk_tag BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id     UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    label  TEXT
);
INSERT INTO tb_tag (label) VALUES ('l1');
CREATE TABLE tv_tag AS
SELECT pk_tag, id, jsonb_build_object('label', label) AS data
FROM (SELECT pk_tag, id, label FROM tb_tag) s;

ALTER TABLE tb_tag RENAME COLUMN label TO caption;

DO $$ BEGIN
  IF (SELECT definition FROM pg_tview_meta WHERE entity = 'tag') LIKE '%label AS label%'
     OR (SELECT definition FROM pg_tview_meta WHERE entity = 'tag') NOT LIKE '%caption%' THEN
    RAISE EXCEPTION '#81 FAIL: definition after renaming label: %',
      (SELECT definition FROM pg_tview_meta WHERE entity = 'tag');
  END IF;
END $$;

UPDATE tb_tag SET caption = 'l2';
DO $$ BEGIN
  IF (SELECT data->>'label' FROM tv_tag WHERE pk_tag = 1) IS DISTINCT FROM 'l2' THEN
    RAISE EXCEPTION '#81 FAIL: update after renaming label did not propagate';
  END IF;
END $$;

-- ========================================================================
-- Cycle 5: renaming a column no TVIEW reads leaves the metadata untouched
-- ========================================================================
ALTER TABLE tb_author ADD COLUMN bio TEXT;
ALTER TABLE tb_author RENAME COLUMN bio TO about;
DO $$ BEGIN
  IF (SELECT definition FROM pg_tview_meta WHERE entity = 'post') LIKE '%about%' THEN
    RAISE EXCEPTION '#81 FAIL: an unrelated rename changed the definition';
  END IF;
END $$;

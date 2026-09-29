-- Regression test: every CREATE TABLE tv_* AS becomes a TVIEW (#80) with exactly its own
-- SELECT (#95).
--
--   * one simple-query batch (psql `\;`) that also runs CREATE EXTENSION: the hook used to
--     skip the whole batch because the query TEXT contained "create extension";
--   * CTAS inside DO / a function: the outer statement held the reentrancy guard;
--   * multi-statement batch: the converted SELECT swallowed the following statements.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_80_ctas_interception.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;

CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title TEXT
);
CREATE TABLE tb_tag (
    pk_tag INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT
);
CREATE TABLE tb_note (
    pk_note INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    body TEXT
);
INSERT INTO tb_post (title) VALUES ('one'), ('two');
INSERT INTO tb_tag (name) VALUES ('t1');
INSERT INTO tb_note (body) VALUES ('n1');

-- ── Shape 1: CREATE EXTENSION + CTAS in ONE simple-query batch ──────────────────
CREATE EXTENSION pg_tviews \; CREATE TABLE tv_post AS
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;

CREATE FUNCTION _assert(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS
  $$ BEGIN IF NOT ok THEN RAISE EXCEPTION 'FAIL: %', msg; END IF; END $$;

SELECT _assert(EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post'),
               '#80 batch+extension: tv_post not registered');
SELECT _assert(to_regclass('v_post') IS NOT NULL, '#80 batch+extension: v_post missing');
UPDATE tb_post SET title = 'uno' WHERE pk_post = 1;
SELECT _assert((SELECT data->>'title' FROM tv_post WHERE pk_post = 1) = 'uno',
               '#80 batch+extension: refresh does not work');

-- ── Shape 2: two CTAS + a trailing statement in one batch; each gets only its SELECT ─
CREATE TABLE tv_tag AS
    SELECT pk_tag, id, jsonb_build_object('name', name) AS data FROM tb_tag \;
CREATE TABLE tv_note AS
    SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM tb_note \;
SELECT 1;

SELECT _assert(EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'tag')
           AND EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'note'),
               '#95 batch: not both registered');
SELECT _assert(position('tv_note' IN definition) = 0 AND position('tb_note' IN definition) = 0,
               '#95: tv_tag definition swallowed the next statement: ' || definition)
  FROM pg_tview_meta WHERE entity = 'tag';
SELECT _assert(position('SELECT 1' IN definition) = 0,
               '#95: tv_note definition swallowed the trailing statement: ' || definition)
  FROM pg_tview_meta WHERE entity = 'note';
DROP TABLE tv_tag, tv_note;

-- ── Shape 3: CTAS inside DO ───────────────────────────────────────────────────
DO $$ BEGIN
  CREATE TABLE tv_tag AS
      SELECT pk_tag, id, jsonb_build_object('name', name) AS data FROM tb_tag;
END $$;
SELECT _assert(EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'tag')
               AND to_regclass('v_tag') IS NOT NULL, '#80 DO: tv_tag not converted');
DROP TABLE tv_tag;

-- ── Shape 4: CTAS inside a function ─────────────────────────────────────────────
CREATE FUNCTION _mk_note() RETURNS void LANGUAGE plpgsql AS $f$
BEGIN
  CREATE TABLE tv_note AS
      SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM tb_note;
END $f$;
SELECT _mk_note();
SELECT _assert(EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'note')
               AND to_regclass('v_note') IS NOT NULL, '#80 function: tv_note not converted');
SELECT _assert((SELECT count(*) FROM tv_note) = 1, '#80 function: tv_note not populated');

-- ── Shape 5: COMMIT inside a procedure keeps working (guard scoping must not change it) ─
CREATE PROCEDURE _bump_note() LANGUAGE plpgsql AS $p$
BEGIN
  UPDATE tb_note SET body = 'n2';
  COMMIT;
  UPDATE tb_note SET body = 'n3';
END $p$;
CALL _bump_note();
SELECT _assert((SELECT data->>'body' FROM tv_note) = 'n3', '#80 procedure COMMIT: tv_note stale');

-- A tv_* created with a column list (not CTAS) stays a plain table.
CREATE TABLE tv_plain (id int);
SELECT _assert(NOT EXISTS (SELECT 1 FROM pg_tview_meta WHERE table_oid = 'tv_plain'::regclass),
               'plain tv_plain was registered');
DROP TABLE tv_plain;

\echo 'regress_issue_80_ctas_interception: OK'

-- Regression test: DDL interception never destroys or silently keeps a table.
--
-- #79  CREATE TABLE IF NOT EXISTS tv_x AS ... on an existing tv_x used to make the
--      ProcessUtility fallback drain DROP the existing TVIEW (PostgreSQL skipped the
--      create, so nothing consumed the pending SELECT), then fail to re-create it.
-- #82  DROP TABLE [IF EXISTS] tv_x on a tv_* table that is NOT a registered TVIEW was
--      claimed by the hook, which then dropped nothing (silently under IF EXISTS).
--      Registration is now decided per relation (by OID), not by the tv_ prefix.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_79_82_ddl_safety.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP SCHEMA IF EXISTS s1 CASCADE;
DROP SCHEMA IF EXISTS s2 CASCADE;
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title TEXT
);
INSERT INTO tb_post (title) VALUES ('one'), ('two');

CREATE FUNCTION _assert(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS
  $$ BEGIN IF NOT ok THEN RAISE EXCEPTION 'FAIL: %', msg; END IF; END $$;

-- ── #79 case 1: IF NOT EXISTS over an existing TVIEW leaves it untouched ────────
CREATE TABLE tv_post AS
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;
SELECT 'tv_post'::regclass::oid AS oid_before \gset
SELECT table_oid AS meta_before FROM pg_tview_meta WHERE entity = 'post' \gset

CREATE TABLE IF NOT EXISTS tv_post AS
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;

SELECT _assert(to_regclass('tv_post') IS NOT NULL, '#79: tv_post was dropped');
SELECT _assert('tv_post'::regclass::oid = :oid_before, '#79: tv_post was recreated');
SELECT _assert((SELECT count(*) FROM tv_post) = 2, '#79: tv_post lost its rows');
SELECT _assert((SELECT table_oid FROM pg_tview_meta WHERE entity = 'post') = :meta_before,
               '#79: pg_tview_meta no longer matches tv_post');
UPDATE tb_post SET title = 'uno' WHERE pk_post = 1;
SELECT _assert((SELECT data->>'title' FROM tv_post WHERE pk_post = 1) = 'uno',
               '#79: refresh stopped working');

-- ── #79 case 2: a plain (non-TVIEW) tv_plain is untouched ─────────────────────────
CREATE TABLE tv_plain (id int);
INSERT INTO tv_plain VALUES (7);
CREATE TABLE IF NOT EXISTS tv_plain AS
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;
SELECT _assert((SELECT count(*) FROM tv_plain) = 1, '#79: plain tv_plain was replaced');
DROP TABLE tv_plain;

-- ── #79 case 3: a failed CTAS must not leave a pending entry that later drops it ───
DO $$ BEGIN
  BEGIN
    EXECUTE 'CREATE TABLE tv_post AS SELECT 1';
    RAISE EXCEPTION 'expected duplicate-table error';
  EXCEPTION WHEN duplicate_table THEN NULL;
  END;
END $$;
CREATE TABLE tv_other_ctas_probe AS SELECT 1 AS x;   -- any later statement
SELECT _assert('tv_post'::regclass::oid = :oid_before, '#79: stale pending entry dropped tv_post');
DROP TABLE tv_other_ctas_probe;

-- ── #82 case 1: unregistered tv_* table drops like a plain table ─────────────────
CREATE TABLE tv_order (id int);
DROP TABLE IF EXISTS tv_order;
SELECT _assert(to_regclass('tv_order') IS NULL, '#82: DROP IF EXISTS kept tv_order');
CREATE TABLE tv_order (id int);
DROP TABLE tv_order;
SELECT _assert(to_regclass('tv_order') IS NULL, '#82: DROP kept tv_order');

-- ── #82 case 2: missing name under IF EXISTS is a no-op ──────────────────────────
DROP TABLE IF EXISTS tv_nothing;

-- ── #82 case 3: schema qualification picks the right relation ─────────────────────
CREATE SCHEMA s1;
CREATE SCHEMA s2;
CREATE TABLE s1.tv_post (id int);            -- plain, same bare name as the TVIEW entity
DROP TABLE s1.tv_post;
SELECT _assert(to_regclass('s1.tv_post') IS NULL, '#82: s1.tv_post not dropped');
SELECT _assert('tv_post'::regclass::oid = :oid_before,
               '#82: dropping s1.tv_post touched the public TVIEW');
SELECT _assert(EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post'),
               '#82: dropping s1.tv_post unregistered the public TVIEW');

-- ── #82 case 4: mixed list ──────────────────────────────────────────────────────
CREATE TABLE tv_plain2 (id int);
CREATE TABLE plain_other (id int);
DROP TABLE tv_post, tv_plain2, plain_other;
SELECT _assert(to_regclass('tv_post') IS NULL AND to_regclass('tv_plain2') IS NULL
               AND to_regclass('plain_other') IS NULL, '#82: mixed drop left a table');
SELECT _assert(NOT EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post'),
               '#82: mixed drop left the TVIEW registered');
SELECT _assert(to_regclass('v_post') IS NULL, '#82: mixed drop left v_post');

\echo 'regress_issue_79_82_ddl_safety: OK'

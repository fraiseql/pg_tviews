-- DDL operations test for pg_tviews: sequential create/drop, creation inside a
-- transaction, rollback, and metadata consistency.
--
-- Standalone (issue #55): self-contained under psql -v ON_ERROR_STOP=1. Rewritten
-- to the supported tb_<entity>/pk_<entity>/id/data convention. The original file's
-- aggregate summary tview (tv_user_summary) and tview-on-tview (tv_post_with_author)
-- sub-tests were removed: those patterns are not supported by the current refresh
-- model (aggregate TVIEWs tracked in #58; tview-on-tview cascade in #51).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/70_concurrent_ddl.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_meta_test CASCADE;
DROP TABLE IF EXISTS tb_meta_test CASCADE;
DROP TABLE IF EXISTS tv_transaction_test CASCADE;
DROP TABLE IF EXISTS tb_transaction_test CASCADE;
DROP TABLE IF EXISTS tv_post_concurrent CASCADE;
DROP TABLE IF EXISTS tv_user_concurrent CASCADE;
DROP TABLE IF EXISTS tb_post_concurrent CASCADE;
DROP TABLE IF EXISTS tb_user_concurrent CASCADE;

-- Setup base tables (Trinity convention: tb_<entity> with pk_<entity>).
CREATE TABLE tb_user_concurrent (
    pk_user_concurrent BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    name TEXT NOT NULL,
    email TEXT UNIQUE
);
CREATE TABLE tb_post_concurrent (
    pk_post_concurrent BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_user BIGINT NOT NULL REFERENCES tb_user_concurrent(pk_user_concurrent),
    title TEXT NOT NULL,
    content TEXT
);

INSERT INTO tb_user_concurrent (name, email) VALUES
    ('Alice', 'alice@example.com'),
    ('Bob', 'bob@example.com'),
    ('Charlie', 'charlie@example.com');
INSERT INTO tb_post_concurrent (fk_user, title, content) VALUES
    (1, 'Alice Post 1', 'Content 1'),
    (1, 'Alice Post 2', 'Content 2'),
    (2, 'Bob Post 1', 'Content 3'),
    (3, 'Charlie Post 1', 'Content 4');

-- Test 1: Sequential TVIEW creation (CTAS interception path).
CREATE TABLE tv_user_concurrent AS
SELECT
    tb_user_concurrent.pk_user_concurrent,
    tb_user_concurrent.id,
    jsonb_build_object(
        'id', tb_user_concurrent.id,
        'name', tb_user_concurrent.name,
        'email', tb_user_concurrent.email
    ) AS data
FROM tb_user_concurrent;

CREATE TABLE tv_post_concurrent AS
SELECT
    tb_post_concurrent.pk_post_concurrent,
    tb_post_concurrent.id,
    jsonb_build_object(
        'id', tb_post_concurrent.id,
        'title', tb_post_concurrent.title,
        'content', tb_post_concurrent.content,
        'author_id', tb_user_concurrent.id
    ) AS data
FROM tb_post_concurrent
JOIN tb_user_concurrent ON tb_post_concurrent.fk_user = tb_user_concurrent.pk_user_concurrent;

DO $$ BEGIN
  IF (SELECT count(*) FROM tv_user_concurrent) <> 3 THEN
    RAISE EXCEPTION 'Test 1 FAIL: tv_user_concurrent should have 3 rows';
  END IF;
  IF (SELECT count(*) FROM tv_post_concurrent) <> 4 THEN
    RAISE EXCEPTION 'Test 1 FAIL: tv_post_concurrent should have 4 rows';
  END IF;
END $$;

-- Test 2: Sequential DROP of a convention TVIEW.
CREATE TABLE tb_meta_test (
    pk_meta_test BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    label TEXT
);
INSERT INTO tb_meta_test (label) VALUES ('m1'), ('m2');
CREATE TABLE tv_meta_test AS
SELECT pk_meta_test, id, jsonb_build_object('label', label) AS data FROM tb_meta_test;

DO $$ BEGIN
  IF (SELECT count(*) FROM tv_meta_test) <> 2 THEN
    RAISE EXCEPTION 'Test 2 FAIL: tv_meta_test should have 2 rows';
  END IF;
END $$;

SELECT pg_tviews_drop('tv_meta_test', true, true);
DO $$ BEGIN
  IF to_regclass('tv_meta_test') IS NOT NULL THEN
    RAISE EXCEPTION 'Test 2 FAIL: tv_meta_test not dropped';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'meta_test') THEN
    RAISE EXCEPTION 'Test 2 FAIL: stale metadata for meta_test after drop';
  END IF;
END $$;

-- Test 3: TVIEW creation inside a transaction persists after COMMIT.
BEGIN;
    CREATE TABLE tb_transaction_test (
        pk_transaction_test BIGSERIAL PRIMARY KEY,
        id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
        value TEXT
    );
    INSERT INTO tb_transaction_test (value) VALUES ('test1'), ('test2');
    CREATE TABLE tv_transaction_test AS
    SELECT pk_transaction_test, id, jsonb_build_object('value', value) AS data
    FROM tb_transaction_test;
COMMIT;

DO $$ BEGIN
  IF (SELECT count(*) FROM tv_transaction_test) <> 2 THEN
    RAISE EXCEPTION 'Test 3 FAIL: tv_transaction_test should persist 2 rows after COMMIT';
  END IF;
END $$;

-- Test 4: A TVIEW created inside a rolled-back transaction leaves nothing behind.
BEGIN;
    CREATE TABLE tb_rollback_test (
        pk_rollback_test BIGSERIAL PRIMARY KEY,
        id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
        value TEXT
    );
    INSERT INTO tb_rollback_test (value) VALUES ('x');
    CREATE TABLE tv_rollback_test AS
    SELECT pk_rollback_test, id, jsonb_build_object('value', value) AS data
    FROM tb_rollback_test;
ROLLBACK;

DO $$ BEGIN
  IF to_regclass('tv_rollback_test') IS NOT NULL THEN
    RAISE EXCEPTION 'Test 4 FAIL: tv_rollback_test should not exist after ROLLBACK';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'rollback_test') THEN
    RAISE EXCEPTION 'Test 4 FAIL: rollback_test metadata should not persist after ROLLBACK';
  END IF;
END $$;

-- Test 5: Metadata + backing objects are consistent for the surviving tviews.
DO $$ BEGIN
  IF (SELECT count(*) FROM pg_tview_meta
      WHERE entity IN ('user_concurrent', 'post_concurrent')) <> 2 THEN
    RAISE EXCEPTION 'Test 5 FAIL: user_concurrent/post_concurrent not both registered';
  END IF;
  IF (SELECT count(*) FROM pg_views
      WHERE viewname IN ('v_user_concurrent', 'v_post_concurrent')) <> 2 THEN
    RAISE EXCEPTION 'Test 5 FAIL: backing views missing';
  END IF;
  IF (SELECT count(*) FROM pg_trigger WHERE tgname LIKE '%tview%') < 2 THEN
    RAISE EXCEPTION 'Test 5 FAIL: change-tracking triggers missing';
  END IF;
END $$;

-- Cleanup
DROP TABLE IF EXISTS tv_transaction_test CASCADE;
DROP TABLE IF EXISTS tb_transaction_test CASCADE;
DROP TABLE IF EXISTS tv_post_concurrent CASCADE;
DROP TABLE IF EXISTS tv_user_concurrent CASCADE;
DROP TABLE IF EXISTS tb_post_concurrent CASCADE;
DROP TABLE IF EXISTS tb_user_concurrent CASCADE;

SELECT '70 PASS: DDL create/drop/transaction/rollback/metadata consistent' AS result;

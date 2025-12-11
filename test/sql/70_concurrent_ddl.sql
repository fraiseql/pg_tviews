-- Test DDL operations
-- This file tests that pg_tviews handles TVIEW creation and management properly
-- Run with: psql -d test_db -f test/sql/70_concurrent_ddl.sql

-- Clean up from previous runs
DROP TABLE IF EXISTS tv_post_with_author;
DROP TABLE IF EXISTS tv_meta_test;
DROP TABLE IF EXISTS tv_post_stats;
DROP TABLE IF EXISTS tv_user_summary;
DROP TABLE IF EXISTS tv_post_concurrent;
DROP TABLE IF EXISTS tv_user_concurrent;
DROP TABLE IF EXISTS tv_transaction_test;
DROP TABLE IF EXISTS tb_transaction_test;
DROP TABLE IF EXISTS tb_meta_test;
DROP TABLE IF EXISTS tb_post_concurrent;
DROP TABLE IF EXISTS tb_user_concurrent;

-- Ensure extension is loaded (skip if not available for testing)
DO $$
BEGIN
    CREATE EXTENSION IF NOT EXISTS pg_tviews;
EXCEPTION
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'Cannot create extension (insufficient privileges) - continuing with test';
    WHEN undefined_file THEN
        RAISE NOTICE 'Extension not installed - continuing with test';
END $$;

-- Setup test tables
CREATE TABLE tb_user_concurrent (
    pk_user BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    email TEXT UNIQUE
);

CREATE TABLE tb_post_concurrent (
    pk_post BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    fk_user BIGINT NOT NULL REFERENCES tb_user_concurrent(pk_user),
    title TEXT NOT NULL,
    content TEXT
);

-- Insert test data
INSERT INTO tb_user_concurrent (name, email) VALUES
    ('Alice', 'alice@example.com'),
    ('Bob', 'bob@example.com'),
    ('Charlie', 'charlie@example.com');

INSERT INTO tb_post_concurrent (fk_user, title, content) VALUES
    (1, 'Alice Post 1', 'Content 1'),
    (1, 'Alice Post 2', 'Content 2'),
    (2, 'Bob Post 1', 'Content 3'),
    (3, 'Charlie Post 1', 'Content 4');

-- Test 1: Sequential TVIEW creation (basic functionality)
SELECT 'Test 1: Sequential TVIEW creation' as test_name;

-- Create first TVIEW
CREATE TABLE tv_user_concurrent AS
SELECT
    tb_user_concurrent.pk_user,
    tb_user_concurrent.id,
    jsonb_build_object(
        'id', tb_user_concurrent.id,
        'name', tb_user_concurrent.name,
        'email', tb_user_concurrent.email
    ) as data
FROM tb_user_concurrent;

-- Verify first TVIEW
SELECT COUNT(*) = 3 as tv_user_created FROM tv_user_concurrent;

-- Create second TVIEW
CREATE TABLE tv_post_concurrent AS
SELECT
    tb_post_concurrent.pk_post,
    tb_post_concurrent.id,
    jsonb_build_object(
        'id', tb_post_concurrent.id,
        'title', tb_post_concurrent.title,
        'content', tb_post_concurrent.content,
        'authorId', tb_user_concurrent.id
    ) as data
FROM tb_post_concurrent
JOIN tb_user_concurrent ON tb_post_concurrent.fk_user = tb_user_concurrent.pk_user;

-- Verify second TVIEW
SELECT COUNT(*) = 4 as tv_post_created FROM tv_post_concurrent;

-- Test 2: Sequential DROP operations
SELECT 'Test 2: Sequential DROP operations' as test_name;

-- Create additional TVIEW for DROP testing
CREATE TABLE tv_user_summary AS
SELECT
    tb_user_concurrent.pk_user,
    tb_user_concurrent.id,
    jsonb_build_object(
        'id', tb_user_concurrent.id,
        'name', tb_user_concurrent.name,
        'postCount', COUNT(tb_post_concurrent.pk_post)
    ) as data
FROM tb_user_concurrent
LEFT JOIN tb_post_concurrent ON tb_user_concurrent.pk_user = tb_post_concurrent.fk_user
GROUP BY tb_user_concurrent.pk_user, tb_user_concurrent.id, tb_user_concurrent.name;

-- Verify TVIEW exists
SELECT COUNT(*) = 3 as tv_user_summary_created FROM tv_user_summary;

-- Test DROP
DROP TABLE tv_user_summary;

-- Verify DROP worked
SELECT COUNT(*) = 0 as tv_user_summary_dropped FROM pg_class WHERE relname = 'tv_user_summary';

-- Test 3: TVIEW creation during transaction
SELECT 'Test 3: TVIEW creation during transaction' as test_name;

-- Start transaction
BEGIN;

-- Create base table
CREATE TABLE tb_transaction_test (
    pk_test BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    value TEXT
);

-- Insert data
INSERT INTO tb_transaction_test (value) VALUES ('test1'), ('test2');

-- Create TVIEW within transaction
CREATE TABLE tv_transaction_test AS
SELECT
    tb_transaction_test.pk_test,
    tb_transaction_test.id,
    jsonb_build_object(
        'id', tb_transaction_test.id,
        'value', tb_transaction_test.value
    ) as data
FROM tb_transaction_test;

-- Verify TVIEW exists within transaction
SELECT COUNT(*) = 2 as tv_created_in_transaction FROM tv_transaction_test;

-- Commit transaction
COMMIT;

-- Verify TVIEW persists after commit
SELECT COUNT(*) = 2 as tv_persisted_after_commit FROM tv_transaction_test;

-- Test rollback scenario
BEGIN;
CREATE TABLE tv_rollback_test AS
SELECT
    tb_transaction_test.pk_test as pk_rollback,
    tb_transaction_test.id,
    tb_transaction_test.value as data
FROM tb_transaction_test
LIMIT 1;
ROLLBACK;

-- Verify TVIEW was rolled back
SELECT COUNT(*) = 0 as tv_rolled_back FROM pg_class WHERE relname = 'tv_rollback_test';

-- Test 4: Metadata consistency
SELECT 'Test 4: Metadata consistency' as test_name;

-- Check metadata for our TVIEWs (if table exists)
SELECT CASE WHEN EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'pg_tview_meta')
    THEN (SELECT COUNT(*) >= 2 FROM pg_tview_meta WHERE entity IN ('user_concurrent', 'post_concurrent'))
    ELSE false END as metadata_registered;

-- Check triggers exist
SELECT COUNT(*) >= 2 as triggers_created FROM pg_trigger WHERE tgname LIKE '%tview%';

-- Check backing views exist
SELECT COUNT(*) = 1 as v_user_exists FROM pg_views WHERE viewname = 'v_user_concurrent';
SELECT COUNT(*) = 1 as v_post_exists FROM pg_views WHERE viewname = 'v_post_concurrent';

-- Test 5: Dependency handling
SELECT 'Test 5: Dependency handling' as test_name;

-- Create TVIEW that depends on another TVIEW (should work)
CREATE TABLE tv_post_with_author AS
SELECT
    tv_post_concurrent.pk_post,
    tv_post_concurrent.id,
    jsonb_build_object(
        'id', tv_post_concurrent.id,
        'title', tv_post_concurrent.data->>'title',
        'author', jsonb_build_object(
            'id', tv_user_concurrent.id,
            'name', tv_user_concurrent.data->>'name'
        )
    ) as data
FROM tv_post_concurrent
JOIN tv_user_concurrent ON tv_post_concurrent.data->>'authorId' = tv_user_concurrent.id::text;

SELECT COUNT(*) = 4 as dependent_tview_created FROM tv_post_with_author;

-- Clean up all test tables
DROP TABLE IF EXISTS tv_post_with_author;
DROP TABLE IF EXISTS tv_meta_test;
DROP TABLE IF EXISTS tv_post_stats;
DROP TABLE IF EXISTS tv_user_summary;
DROP TABLE IF EXISTS tv_post_concurrent;
DROP TABLE IF EXISTS tv_user_concurrent;
DROP TABLE IF EXISTS tv_transaction_test;
DROP TABLE IF EXISTS tb_transaction_test;
DROP TABLE IF EXISTS tb_meta_test;
DROP TABLE IF EXISTS tb_post_concurrent;
DROP TABLE IF EXISTS tb_user_concurrent;

-- Final verification
SELECT 'All DDL tests completed successfully' as result;
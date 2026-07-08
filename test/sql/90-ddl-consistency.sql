-- Standalone preamble (issue #55): make this file self-contained so it passes
-- on its own in a fresh database under psql -v ON_ERROR_STOP=1.
\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
-- Test DDL Syntax Consistency
-- Verify CREATE TABLE tv_ AS SELECT ... and pg_tviews_create() produce identical results

-- Clean up any existing test data
SELECT pg_tviews_drop('ddl_user') WHERE EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'ddl_user');
SELECT pg_tviews_drop('fn_user') WHERE EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'fn_user');
DROP TABLE IF EXISTS tb_ddl_user CASCADE;
DROP TABLE IF EXISTS tb_fn_user CASCADE;
DROP VIEW IF EXISTS v_ddl_user CASCADE;
DROP VIEW IF EXISTS v_fn_user CASCADE;
DROP TABLE IF EXISTS tv_ddl_user CASCADE;
DROP TABLE IF EXISTS tv_fn_user CASCADE;

-- Create two identical base tables (entity derived from pk_<entity>)
CREATE TABLE tb_ddl_user (
    pk_ddl_user BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT NOT NULL,
    email TEXT,
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE tb_fn_user (
    pk_fn_user BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT NOT NULL,
    email TEXT,
    created_at TIMESTAMPTZ DEFAULT now()
);

-- Insert identical test data
INSERT INTO tb_ddl_user (pk_ddl_user, name, email) VALUES
    (1, 'Alice', 'alice@example.com'),
    (2, 'Bob', 'bob@example.com');

INSERT INTO tb_fn_user (pk_fn_user, name, email) VALUES
    (1, 'Alice', 'alice@example.com'),
    (2, 'Bob', 'bob@example.com');

-- Method 1: CREATE TABLE tv_ AS SELECT ... DDL (intercepted by event trigger)
CREATE TABLE tv_ddl_user AS
SELECT
    pk_ddl_user,
    id,
    jsonb_build_object(
        'id', id,
        'name', name,
        'email', email,
        'created_at', created_at
    ) as data
FROM tb_ddl_user;

-- Method 2: pg_tviews_create() function
SELECT pg_tviews_create('fn_user', $$
SELECT
    pk_fn_user,
    id,
    jsonb_build_object(
        'id', id,
        'name', name,
        'email', email,
        'created_at', created_at
    ) as data
FROM tb_fn_user
$$);

-- Compare results
SELECT 'DDL Method Results:' as method;
SELECT * FROM tv_ddl_user ORDER BY pk_ddl_user;

SELECT 'Function Method Results:' as method;
SELECT * FROM tv_fn_user ORDER BY pk_fn_user;

-- Check metadata: both should have entries
SELECT 'Metadata Comparison:' as comparison;
SELECT
    entity,
    view_oid,
    table_oid,
    array_length(cascade_paths, 1) as cascade_count
FROM pg_tview_meta
WHERE entity IN ('ddl_user', 'fn_user')
ORDER BY entity;

-- Verify both methods created the same number of rows
DO $$
DECLARE
    ddl_count INTEGER;
    fn_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO ddl_count FROM tv_ddl_user;
    SELECT COUNT(*) INTO fn_count FROM tv_fn_user;

    IF ddl_count != fn_count THEN
        RAISE EXCEPTION 'Row count mismatch: DDL=%, function=%', ddl_count, fn_count;
    END IF;

    IF ddl_count != 2 THEN
        RAISE EXCEPTION 'Expected 2 rows each, got %', ddl_count;
    END IF;

    RAISE NOTICE 'Both methods produced % rows', ddl_count;
END $$;

-- Verify both have metadata entries
DO $$
DECLARE
    meta_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO meta_count
    FROM pg_tview_meta
    WHERE entity IN ('ddl_user', 'fn_user');

    IF meta_count != 2 THEN
        RAISE EXCEPTION 'Expected 2 metadata entries, got %', meta_count;
    END IF;

    RAISE NOTICE 'Both methods created metadata entries';
END $$;

-- Check views exist
SELECT 'View Comparison:' as comparison;
SELECT
    schemaname,
    viewname
FROM pg_views
WHERE viewname IN ('v_ddl_user', 'v_fn_user')
ORDER BY viewname;

-- Clean up
SELECT pg_tviews_drop('ddl_user');
SELECT pg_tviews_drop('fn_user');
DROP TABLE IF EXISTS tb_ddl_user CASCADE;
DROP TABLE IF EXISTS tb_fn_user CASCADE;

-- Test DDL Syntax Consistency
-- Verify CREATE TABLE tv_ AS SELECT ... and pg_tviews_create() produce identical results

-- Clean up any existing test data
DROP TABLE IF EXISTS tb_test_user CASCADE;
DROP VIEW IF EXISTS v_test_user CASCADE;
DROP TABLE IF EXISTS tv_test_user1 CASCADE;
DROP TABLE IF EXISTS tv_test_user2 CASCADE;

-- Create test base table
CREATE TABLE tb_test_user (
    pk_test_user BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    email TEXT,
    created_at TIMESTAMPTZ DEFAULT now()
);

-- Insert test data
INSERT INTO tb_test_user (name, email) VALUES 
    ('Alice', 'alice@example.com'),
    ('Bob', 'bob@example.com');

-- Method 1: CREATE TABLE tv_ AS SELECT ... DDL (intercepted by ProcessUtility hook)
CREATE TABLE tv_test_user1 AS
SELECT
    pk_test_user as pk_test_user,
    id,
    name,
    jsonb_build_object(
        'id', id,
        'name', name,
        'email', email,
        'createdAt', created_at
    ) as data
FROM tb_test_user;

-- Method 2: pg_tviews_create() function
SELECT pg_tviews_create('tv_test_user2', '
SELECT
    pk_test_user as pk_test_user,
    id,
    name,
    jsonb_build_object(
        ''id'', id,
        ''name'', name,
        ''email'', email,
        ''createdAt'', created_at
    ) as data
FROM tb_test_user
');

-- Compare results
SELECT 'DDL Method Results:' as method;
SELECT * FROM tv_test_user1 ORDER BY pk_test_user;

SELECT 'Function Method Results:' as method;
SELECT * FROM tv_test_user2 ORDER BY pk_test_user;

-- Check metadata
SELECT 'Metadata Comparison:' as comparison;
SELECT 
    entity,
    view_oid,
    table_oid,
    array_length(dependencies, 1) as dep_count
FROM pg_tview_meta 
WHERE entity = 'test_user1' OR entity = 'test_user2'
ORDER BY entity;

-- Check triggers
SELECT 'Trigger Comparison:' as comparison;
SELECT 
    tgname,
    tgrelid::regclass::text as table_name,
    obj_description(tgrelid, 'pg_class') as table_comment
FROM pg_trigger 
WHERE tgname LIKE '%test_user%'
ORDER BY tgname;

-- Check views
SELECT 'View Comparison:' as comparison;
SELECT 
    schemaname,
    viewname,
    definition 
FROM pg_views 
WHERE viewname LIKE '%test_user%'
ORDER BY viewname;

-- Clean up
DROP TABLE IF EXISTS tb_test_user CASCADE;
DROP VIEW IF EXISTS v_test_user CASCADE;
DROP TABLE IF EXISTS tv_test_user1 CASCADE;
DROP TABLE IF EXISTS tv_test_user2 CASCADE;

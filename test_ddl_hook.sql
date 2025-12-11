-- Test if ProcessUtility hook intercepts CREATE TABLE tv_* AS SELECT

-- Clean up
DROP TABLE IF EXISTS tb_test_user CASCADE;
DROP VIEW IF EXISTS v_test_user CASCADE;
DROP TABLE IF EXISTS tv_test_user CASCADE;

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

-- Try CREATE TABLE tv_* AS SELECT (what the hook looks for)
CREATE TABLE tv_test_user AS
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

-- Check if it was converted to TVIEW
SELECT 'TVIEW created?' as check;
SELECT * FROM tv_test_user ORDER BY pk_test_user;

-- Check metadata
SELECT 'Metadata:' as check;
SELECT entity, view_oid, table_oid FROM pg_tview_meta WHERE entity = 'test_user';

-- Check triggers
SELECT 'Triggers:' as check;
SELECT tgname, tgrelid::regclass::text as table_name FROM pg_trigger WHERE tgname LIKE '%test_user%';

-- Clean up
DROP TABLE IF EXISTS tb_test_user CASCADE;
DROP VIEW IF EXISTS v_test_user CASCADE;
DROP TABLE IF EXISTS tv_test_user CASCADE;

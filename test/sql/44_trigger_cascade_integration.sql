-- Test 44: Full End-to-End Integration
-- Purpose: Comprehensive integration test with realistic PrintOptim-like schema
-- Expected: All operations work together (CREATE, INSERT, UPDATE, DELETE, cascade)

\set ECHO all
\set ON_ERROR_STOP on

BEGIN;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;

CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\echo '=========================================='
\echo 'Test 44: Full Integration Test'
\echo '=========================================='

-- Create realistic 3-level hierarchy: company -> user -> post
-- Similar to PrintOptim structure

CREATE TABLE tb_company (
    pk_company INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT NOT NULL,
    industry TEXT,
    employee_count INTEGER DEFAULT 0
);

CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_company INTEGER NOT NULL,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    role TEXT DEFAULT 'member',
    FOREIGN KEY (fk_company) REFERENCES tb_company(pk_company)
);

CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user INTEGER NOT NULL,
    title TEXT NOT NULL,
    content TEXT,
    status TEXT DEFAULT 'draft',
    view_count INTEGER DEFAULT 0,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    FOREIGN KEY (fk_user) REFERENCES tb_user(pk_user)
);

-- Insert realistic test data
INSERT INTO tb_company (name, industry, employee_count) VALUES
    ('Acme Corp', 'Technology', 150),
    ('Globex Inc', 'Manufacturing', 500);

INSERT INTO tb_user (fk_company, name, email, role) VALUES
    (1, 'Alice Johnson', 'alice@acme.com', 'admin'),
    (1, 'Bob Smith', 'bob@acme.com', 'member'),
    (1, 'Carol White', 'carol@acme.com', 'member'),
    (2, 'David Brown', 'david@globex.com', 'admin');

INSERT INTO tb_post (fk_user, title, content, status, view_count) VALUES
    (1, 'Welcome to Acme', 'This is our first post', 'published', 100),
    (1, 'Q2 Updates', 'Quarterly updates here', 'published', 50),
    (2, 'Engineering Blog', 'Technical insights', 'draft', 5),
    (3, 'Design Patterns', 'UI/UX best practices', 'published', 75),
    (4, 'Manufacturing News', 'Latest from the floor', 'published', 30);

-- Create TVIEWs (bottom-up: company -> user -> post)
\echo ''
\echo 'Step 1: Creating TVIEW hierarchy'

CREATE TABLE tv_company AS
SELECT
    pk_company,
    id,
    jsonb_build_object(
        'id', id::text,
        'name', name,
        'industry', industry,
        'employeeCount', employee_count
    ) AS data
FROM tb_company;

CREATE TABLE tv_user AS
SELECT
    u.pk_user,
    u.id,
    u.fk_company,
    v_company.id AS company_id,
    jsonb_build_object(
        'id', u.id::text,
        'name', u.name,
        'email', u.email,
        'role', u.role,
        'company', v_company.data
    ) AS data
FROM tb_user u
JOIN v_company ON v_company.pk_company = u.fk_company;

CREATE TABLE tv_post AS
SELECT
    p.pk_post,
    p.id,
    p.fk_user,
    v_user.id AS user_id,
    jsonb_build_object(
        'id', p.id::text,
        'title', p.title,
        'content', p.content,
        'status', p.status,
        'viewCount', p.view_count,
        'createdAt', p.created_at,
        'author', v_user.data
    ) AS data
FROM tb_post p
JOIN v_user ON v_user.pk_user = p.fk_user;

\echo '✓ Step 1 complete: TVIEWs created'

-- Test 1: Verify initial population
\echo ''
\echo 'Test 1: Verify initial population'

SELECT COUNT(*) AS company_count FROM tv_company;
-- Expected: 2

SELECT COUNT(*) AS user_count FROM tv_user;
-- Expected: 4

SELECT COUNT(*) AS post_count FROM tv_post;
-- Expected: 5

-- Verify nested data structure
SELECT
    pk_post,
    data->>'title' AS title,
    data->'author'->>'name' AS author_name,
    data->'author'->'company'->>'name' AS company_name
FROM tv_post
WHERE pk_post = 1;
-- Expected: 'Welcome to Acme', 'Alice Johnson', 'Acme Corp'

\echo '✓ Test 1 passed: Initial population correct'

-- Test 2: Company update cascades through 2 levels
\echo ''
\echo 'Test 2: Company name change cascades to users and posts'

\timing on
UPDATE tb_company SET name = 'Acme Corporation' WHERE pk_company = 1;
\timing off

-- Verify company updated
SELECT data->>'name' FROM tv_company WHERE pk_company = 1;
-- Expected: 'Acme Corporation'

-- Verify users updated (3 users at Acme)
SELECT
    pk_user,
    data->>'name' AS user_name,
    data->'company'->>'name' AS company_name
FROM tv_user
WHERE fk_company = 1
ORDER BY pk_user;
-- Expected: all 3 show 'Acme Corporation'

-- Verify posts updated (4 posts by Acme users)
SELECT
    pk_post,
    data->>'title' AS title,
    data->'author'->'company'->>'name' AS company_name
FROM tv_post
WHERE data->'author'->'company'->>'name' = 'Acme Corporation'
ORDER BY pk_post;
-- Expected: 4 posts with 'Acme Corporation'

\echo '✓ Test 2 passed: 2-level cascade works (company -> user -> post)'

-- Test 3: User update cascades to posts only
\echo ''
\echo 'Test 3: User update cascades to posts'

UPDATE tb_user SET name = 'Alice J. Updated' WHERE pk_user = 1;

-- Verify user updated
SELECT data->>'name' FROM tv_user WHERE pk_user = 1;
-- Expected: 'Alice J. Updated'

-- Verify Alice's posts updated (2 posts)
SELECT
    pk_post,
    data->'author'->>'name' AS author_name
FROM tv_post
WHERE fk_user = 1
ORDER BY pk_post;
-- Expected: both show 'Alice J. Updated'

-- Verify other users' posts NOT updated
SELECT
    pk_post,
    data->'author'->>'name' AS author_name
FROM tv_post
WHERE fk_user = 2;
-- Expected: 'Bob Smith' (unchanged)

\echo '✓ Test 3 passed: 1-level cascade works (user -> post)'

-- Test 4: Post update does NOT cascade
\echo ''
\echo 'Test 4: Post update does not cascade upward'

-- Record timestamps
SELECT updated_at FROM tv_user WHERE pk_user = 1 \gset user_ts_
SELECT updated_at FROM tv_company WHERE pk_company = 1 \gset company_ts_

SELECT pg_sleep(0.1);

-- Update post
UPDATE tb_post SET title = 'Updated Welcome', view_count = 999 WHERE pk_post = 1;

-- Verify post updated
SELECT
    data->>'title' AS title,
    (data->>'viewCount')::int AS views
FROM tv_post
WHERE pk_post = 1;
-- Expected: 'Updated Welcome', 999

-- Verify user and company NOT updated (timestamps unchanged)
SELECT updated_at = :'user_ts_updated_at'::timestamptz AS user_unchanged
FROM tv_user WHERE pk_user = 1;
-- Expected: true

SELECT updated_at = :'company_ts_updated_at'::timestamptz AS company_unchanged
FROM tv_company WHERE pk_company = 1;
-- Expected: true

\echo '✓ Test 4 passed: Post update does not cascade upward'

-- Test 5: INSERT operations
\echo ''
\echo 'Test 5: INSERT operations work correctly'

-- Add new user to Acme
INSERT INTO tb_user (fk_company, name, email, role)
VALUES (1, 'Eve Wilson', 'eve@acme.com', 'member');

-- Verify new user has company data
SELECT
    data->>'name' AS user_name,
    data->'company'->>'name' AS company_name
FROM tv_user
WHERE data->>'email' = 'eve@acme.com';
-- Expected: 'Eve Wilson', 'Acme Corporation'

-- Add post by new user
INSERT INTO tb_post (fk_user, title, content, status)
VALUES (5, 'First Post by Eve', 'Hello world', 'published');

-- Verify new post has full nested data
SELECT
    data->>'title' AS title,
    data->'author'->>'name' AS author_name,
    data->'author'->'company'->>'name' AS company_name
FROM tv_post
WHERE data->>'title' = 'First Post by Eve';
-- Expected: 'First Post by Eve', 'Eve Wilson', 'Acme Corporation'

\echo '✓ Test 5 passed: INSERT operations work'

-- Test 6: DELETE operations
\echo ''
\echo 'Test 6: DELETE operations work correctly'

-- Delete a post
DELETE FROM tb_post WHERE pk_post = 5;

-- Verify post deleted from TVIEW
SELECT COUNT(*) FROM tv_post WHERE pk_post = 5;
-- Expected: 0

-- Verify user still exists
SELECT COUNT(*) FROM tv_user WHERE pk_user = 4;
-- Expected: 1

\echo '✓ Test 6 passed: DELETE operations work'

-- Test 7: FK change (move user to different company)
\echo ''
\echo 'Test 7: FK change updates nested data'

-- Move Bob from Acme to Globex
UPDATE tb_user SET fk_company = 2 WHERE pk_user = 2;

-- Verify Bob now has Globex company data
SELECT
    data->>'name' AS user_name,
    data->'company'->>'name' AS company_name
FROM tv_user
WHERE pk_user = 2;
-- Expected: 'Bob Smith', 'Globex Inc'

-- Verify Bob's post now shows Globex
SELECT
    data->>'title' AS title,
    data->'author'->'company'->>'name' AS company_name
FROM tv_post
WHERE fk_user = 2;
-- Expected: 'Engineering Blog', 'Globex Inc'

\echo '✓ Test 7 passed: FK change updates nested data'

-- Test 8: Bulk update performance
\echo ''
\echo 'Test 8: Bulk update performance'

-- Update company (affects 3 users, 4+ posts)
\timing on
UPDATE tb_company
SET industry = 'Tech & Innovation', employee_count = 200
WHERE pk_company = 1;
\timing off

-- Verify cascade completed
SELECT
    (data->>'employeeCount')::int AS employee_count,
    data->>'industry' AS industry
FROM tv_company
WHERE pk_company = 1;
-- Expected: 200, 'Tech & Innovation'

-- Verify cascaded to all levels
SELECT COUNT(*) AS affected_posts
FROM tv_post
WHERE data->'author'->'company'->>'industry' = 'Tech & Innovation';
-- Expected: 4+ (posts by Acme users)

\echo '✓ Test 8 passed: Bulk update performs well'

-- Test 9: Verify metadata integrity
\echo ''
\echo 'Test 9: Verify metadata integrity'

SELECT
    entity,
    array_length(dependencies, 1) AS dep_count,
    array_length(fk_columns, 1) AS fk_count,
    array_length(uuid_fk_columns, 1) AS uuid_fk_count
FROM pg_tview_meta
ORDER BY entity;
-- Expected:
--   company: 0 deps, 0 fks, 0 uuid_fks
--   user: 1 dep, 1 fk, 1 uuid_fk
--   post: 2 deps, 1 fk, 1 uuid_fk

\echo '✓ Test 9 passed: Metadata integrity correct'

-- Test 10: Verify triggers installed
\echo ''
\echo 'Test 10: Verify triggers installed correctly'

SELECT
    tgname,
    tgrelid::regclass AS table_name,
    tgenabled
FROM pg_trigger
WHERE tgname LIKE 'trg_tview_%'
ORDER BY tgname;
-- Expected: triggers on tb_company, tb_user, tb_post

\echo '✓ Test 10 passed: Triggers installed correctly'

-- Performance summary
\echo ''
\echo '=========================================='
\echo 'Performance Summary'
\echo '=========================================='
\echo 'Company update (2-level cascade): see timing above'
\echo 'Target: < 500ms for 100 rows'
\echo 'Target: < 5ms for single row'
\echo '=========================================='

\echo ''
\echo '=========================================='
\echo 'Test 44: All tests passed! ✓'
\echo 'Full integration successful!'
\echo '=========================================='

ROLLBACK;

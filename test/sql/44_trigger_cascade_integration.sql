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
\ir lib/assert_fresh.sql

-- All three TVIEWs must match their backing views.
CREATE FUNCTION assert_all_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM assert_fresh('tv_company', 'pk_company', label);
    PERFORM assert_fresh('tv_user', 'pk_user', label);
    PERFORM assert_fresh('tv_post', 'pk_post', label);
END $$;

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
        'employee_count', employee_count
    ) AS data
FROM tb_company;

CREATE TABLE tv_user AS
SELECT
    u.pk_user,
    u.id,
    u.fk_company,
    tv_company.id AS company_id,
    jsonb_build_object(
        'id', u.id::text,
        'name', u.name,
        'email', u.email,
        'role', u.role,
        'company', tv_company.data
    ) AS data
FROM tb_user u
JOIN tv_company ON tv_company.pk_company = u.fk_company;

CREATE TABLE tv_post AS
SELECT
    p.pk_post,
    p.id,
    p.fk_user,
    tv_user.id AS user_id,
    jsonb_build_object(
        'id', p.id::text,
        'title', p.title,
        'content', p.content,
        'status', p.status,
        'view_count', p.view_count,
        'created_at', p.created_at,
        'author', tv_user.data
    ) AS data
FROM tb_post p
JOIN tv_user ON tv_user.pk_user = p.fk_user;

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

SELECT assert_all_fresh('creating the hierarchy');
DO $$ BEGIN
    IF (SELECT COUNT(*) FROM tv_company) <> 2 OR (SELECT COUNT(*) FROM tv_user) <> 4
       OR (SELECT COUNT(*) FROM tv_post) <> 5 THEN
        RAISE EXCEPTION 'FAIL: initial row counts not 2/4/5';
    END IF;
    IF (SELECT (data->>'title', data->'author'->>'name', data->'author'->'company'->>'name')
        FROM tv_post WHERE pk_post = 1)
       IS DISTINCT FROM ('Welcome to Acme'::text, 'Alice Johnson'::text, 'Acme Corp'::text) THEN
        RAISE EXCEPTION 'FAIL: post 1 nested author/company wrong';
    END IF;
END $$;

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

SELECT assert_all_fresh('an UPDATE of tb_company.name');
DO $$ BEGIN
    IF (SELECT data->>'name' FROM tv_company WHERE pk_company = 1) IS DISTINCT FROM 'Acme Corporation' THEN
        RAISE EXCEPTION 'FAIL: tv_company row 1 not renamed';
    END IF;
    IF (SELECT COUNT(*) FROM tv_user WHERE fk_company = 1 AND data->'company'->>'name' = 'Acme Corporation') <> 3 THEN
        RAISE EXCEPTION 'FAIL: rename did not reach all 3 Acme users';
    END IF;
    IF (SELECT COUNT(*) FROM tv_post WHERE data->'author'->'company'->>'name' = 'Acme Corporation') <> 4 THEN
        RAISE EXCEPTION 'FAIL: rename did not reach all 4 Acme posts';
    END IF;
END $$;

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

SELECT assert_all_fresh('an UPDATE of tb_user.name');
DO $$ BEGIN
    IF (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) IS DISTINCT FROM 'Alice J. Updated' THEN
        RAISE EXCEPTION 'FAIL: tv_user row 1 not renamed';
    END IF;
    IF (SELECT COUNT(*) FROM tv_post WHERE fk_user = 1 AND data->'author'->>'name' = 'Alice J. Updated') <> 2 THEN
        RAISE EXCEPTION 'FAIL: rename did not reach both of Alice''s posts';
    END IF;
    IF (SELECT data->'author'->>'name' FROM tv_post WHERE fk_user = 2) IS DISTINCT FROM 'Bob Smith' THEN
        RAISE EXCEPTION 'FAIL: Bob''s post changed by Alice''s rename';
    END IF;
END $$;

\echo '✓ Test 3 passed: 1-level cascade works (user -> post)'

-- Test 4: Post update does NOT cascade
\echo ''
\echo 'Test 4: Post update does not cascade upward'

-- Record tuple ids: the file runs in one transaction, where updated_at
-- (now()) cannot change, but any rewrite of a row gives it a new ctid.
CREATE TEMP TABLE before_post_update AS
SELECT (SELECT ctid FROM tv_user WHERE pk_user = 1) AS user_tid,
       (SELECT ctid FROM tv_company WHERE pk_company = 1) AS company_tid;

-- Update post
UPDATE tb_post SET title = 'Updated Welcome', view_count = 999 WHERE pk_post = 1;

-- Verify post updated
SELECT
    data->>'title' AS title,
    (data->>'view_count')::int AS views
FROM tv_post
WHERE pk_post = 1;
-- Expected: 'Updated Welcome', 999

-- Verify user and company NOT updated (rows not rewritten)
SELECT ctid = (SELECT user_tid FROM before_post_update) AS user_unchanged
FROM tv_user WHERE pk_user = 1;
-- Expected: true

SELECT ctid = (SELECT company_tid FROM before_post_update) AS company_unchanged
FROM tv_company WHERE pk_company = 1;
-- Expected: true

SELECT assert_all_fresh('an UPDATE of tb_post');
DO $$ BEGIN
    IF (SELECT (data->>'title', (data->>'view_count')::int) FROM tv_post WHERE pk_post = 1)
       IS DISTINCT FROM ('Updated Welcome'::text, 999) THEN
        RAISE EXCEPTION 'FAIL: post 1 not (Updated Welcome, 999)';
    END IF;
    IF (SELECT ctid FROM tv_user WHERE pk_user = 1) IS DISTINCT FROM (SELECT user_tid FROM before_post_update)
       OR (SELECT ctid FROM tv_company WHERE pk_company = 1) IS DISTINCT FROM (SELECT company_tid FROM before_post_update) THEN
        RAISE EXCEPTION 'FAIL: a post update rewrote its user or company row';
    END IF;
END $$;

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

SELECT assert_all_fresh('an INSERT into tb_user');
DO $$ BEGIN
    IF (SELECT data->'company'->>'name' FROM tv_user WHERE data->>'email' = 'eve@acme.com')
       IS DISTINCT FROM 'Acme Corporation' THEN
        RAISE EXCEPTION 'FAIL: new user Eve does not embed Acme Corporation';
    END IF;
END $$;

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

SELECT assert_all_fresh('an INSERT into tb_post');
DO $$ BEGIN
    IF (SELECT (data->'author'->>'name', data->'author'->'company'->>'name') FROM tv_post
        WHERE data->>'title' = 'First Post by Eve')
       IS DISTINCT FROM ('Eve Wilson'::text, 'Acme Corporation'::text) THEN
        RAISE EXCEPTION 'FAIL: Eve''s post nested author/company wrong';
    END IF;
END $$;

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

SELECT assert_all_fresh('a DELETE from tb_post');
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tv_post WHERE pk_post = 5) THEN
        RAISE EXCEPTION 'FAIL: deleted post 5 still in tv_post';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM tv_user WHERE pk_user = 4) THEN
        RAISE EXCEPTION 'FAIL: deleting a post removed its user';
    END IF;
END $$;

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

SELECT assert_all_fresh('re-pointing tb_user.fk_company');
DO $$ BEGIN
    IF (SELECT data->'company'->>'name' FROM tv_user WHERE pk_user = 2) IS DISTINCT FROM 'Globex Inc' THEN
        RAISE EXCEPTION 'FAIL: moved user Bob does not embed Globex Inc';
    END IF;
    IF (SELECT data->'author'->'company'->>'name' FROM tv_post WHERE fk_user = 2) IS DISTINCT FROM 'Globex Inc' THEN
        RAISE EXCEPTION 'FAIL: Bob''s post does not show Globex Inc';
    END IF;
END $$;

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
    (data->>'employee_count')::int AS employee_count,
    data->>'industry' AS industry
FROM tv_company
WHERE pk_company = 1;
-- Expected: 200, 'Tech & Innovation'

-- Verify cascaded to all levels
SELECT COUNT(*) AS affected_posts
FROM tv_post
WHERE data->'author'->'company'->>'industry' = 'Tech & Innovation';
-- Expected: 4 (posts 1, 2, 4 and Eve's; Bob's moved to Globex, post 5 deleted)

SELECT assert_all_fresh('an UPDATE of two tb_company fields');
DO $$ BEGIN
    IF (SELECT ((data->>'employee_count')::int, data->>'industry') FROM tv_company WHERE pk_company = 1)
       IS DISTINCT FROM (200, 'Tech & Innovation'::text) THEN
        RAISE EXCEPTION 'FAIL: tv_company row 1 not (200, Tech & Innovation)';
    END IF;
    IF (SELECT COUNT(*) FROM tv_post WHERE data->'author'->'company'->>'industry' = 'Tech & Innovation') <> 4 THEN
        RAISE EXCEPTION 'FAIL: industry change did not reach the 4 Acme posts';
    END IF;
END $$;

\echo '✓ Test 8 passed: Bulk update performs well'

-- Test 9: Verify metadata integrity
\echo ''
\echo 'Test 9: Verify metadata integrity'

SELECT
    entity,
    jsonb_array_length(plan->'paths') AS local_paths,
    jsonb_array_length(plan->'embeds') AS embeds
FROM pg_tview_meta
ORDER BY entity;
-- Expected: one local path each; user embeds tv_company and post embeds
-- tv_user (their definitions join those TVIEWs), company embeds nothing.

DO $$ BEGIN
    IF (SELECT string_agg(format('%s:%s/%s', entity, jsonb_array_length(plan->'paths'),
                                 jsonb_array_length(plan->'embeds')), ',' ORDER BY entity)
        FROM pg_tview_meta)
       IS DISTINCT FROM 'company:1/0,post:1/1,user:1/1' THEN
        RAISE EXCEPTION 'FAIL: unexpected local paths/embeds per TVIEW';
    END IF;
END $$;

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

-- Each base table carries an enabled row trigger and statement-level flush trigger.
DO $$
DECLARE t regclass;
BEGIN
    FOREACH t IN ARRAY ARRAY['tb_company'::regclass, 'tb_user'::regclass, 'tb_post'::regclass] LOOP
        IF NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = t AND tgname LIKE 'trg_tview_row_%' AND tgenabled = 'O')
           OR NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = t AND tgname LIKE 'trg_tview_flush_%' AND tgenabled = 'O') THEN
            RAISE EXCEPTION 'FAIL: row or flush trigger missing on %', t;
        END IF;
    END LOOP;
END $$;

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

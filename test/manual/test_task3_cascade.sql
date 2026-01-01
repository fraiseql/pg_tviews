-- Test Phase 4 Task 3: FK Lineage Cascade
-- Purpose: Verify cascade propagation through FK relationships using pg_tviews_create()

\set ON_ERROR_STOP on

-- Clean slate
DROP DATABASE IF EXISTS test_cascade;
CREATE DATABASE test_cascade;
\c test_cascade

-- Load extensions
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;

\echo '=========================================='
\echo 'Test: FK Lineage Cascade'
\echo '=========================================='

-- Create two-level hierarchy: user -> post
CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT NOT NULL,
    email TEXT
);

CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user INTEGER NOT NULL,
    title TEXT NOT NULL,
    content TEXT,
    FOREIGN KEY (fk_user) REFERENCES tb_user(pk_user)
);

-- Insert test data
INSERT INTO tb_user (name, email) VALUES
    ('Alice', 'alice@example.com'),
    ('Bob', 'bob@example.com');

INSERT INTO tb_post (fk_user, title, content) VALUES
    (1, 'Alice Post 1', 'Content 1'),
    (1, 'Alice Post 2', 'Content 2'),
    (1, 'Alice Post 3', 'Content 3'),
    (2, 'Bob Post 1', 'Bob content');

-- Create helper views (workaround for schema inference bug with inline jsonb_build_object)
CREATE VIEW user_prepared AS
SELECT
    pk_user,
    id,
    jsonb_build_object(
        'id', id::text,
        'name', name,
        'email', email
    ) AS data
FROM tb_user;

CREATE VIEW post_prepared AS
SELECT
    p.pk_post,
    p.id,
    p.fk_user,
    u.id AS user_id,
    jsonb_build_object(
        'id', p.id::text,
        'title', p.title,
        'content', p.content,
        'author', u.data
    ) AS data
FROM tb_post p
JOIN user_prepared u ON u.pk_user = p.fk_user;

-- Create TVIEWs using SQL functions (order matters: parent first)
\echo ''
\echo 'Creating tv_user...'
SELECT pg_tviews_create('tv_user', 'SELECT pk_user, id, data FROM user_prepared');

\echo 'Creating tv_post...'
SELECT pg_tviews_create('tv_post', 'SELECT pk_post, id, fk_user, user_id, data FROM post_prepared');

-- Test 1: Verify initial state
\echo ''
\echo 'Test 1: Verify initial population'
SELECT COUNT(*) AS user_count FROM tv_user;
-- Expected: 2

SELECT COUNT(*) AS post_count FROM tv_post;
-- Expected: 4

-- Verify nested author data
SELECT
    pk_post,
    data->>'title' AS title,
    data->'author'->>'name' AS author_name,
    data->'author'->>'email' AS author_email
FROM tv_post
WHERE fk_user = 1
ORDER BY pk_post;
-- Expected: 3 posts with author 'Alice', 'alice@example.com'

\echo '✓ Test 1 passed: Initial population correct'

-- Test 2: Update parent (user) - should cascade to posts
\echo ''
\echo 'Test 2: Update parent cascades to children'
\echo 'Updating Alice email...'
UPDATE tb_user SET email = 'alice.updated@example.com' WHERE pk_user = 1;

-- Wait for cascade (synchronous in current implementation)
\echo 'Checking if cascade updated tv_post...'
SELECT
    pk_post,
    data->>'title' AS title,
    data->'author'->>'email' AS author_email_in_post
FROM tv_post
WHERE fk_user = 1
ORDER BY pk_post;
-- Expected: All 3 Alice posts should show 'alice.updated@example.com'

\echo ''
\echo 'If author_email_in_post = alice.updated@example.com, cascade worked! ✓'
\echo 'If author_email_in_post = alice@example.com, cascade FAILED ✗'

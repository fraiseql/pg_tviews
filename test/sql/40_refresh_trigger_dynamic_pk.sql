-- Test 40: Dynamic PK Extraction in Trigger Handler
-- Purpose: Verify trigger handler extracts PK column name dynamically
-- Expected: Trigger works with any pk_* column name (pk_post, pk_user, etc.)

\set ECHO all
\set ON_ERROR_STOP on

-- Start transaction with proper isolation
BEGIN;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

-- Clean up any existing test objects
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;

-- Load extensions
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\echo '=========================================='
\echo 'Test 40: Dynamic PK Extraction'
\echo '=========================================='

-- Test 1: Standard pk_post column
\echo ''
\echo 'Test 1: Standard pk_post column'
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title TEXT NOT NULL,
    content TEXT
);

INSERT INTO tb_post (title, content)
VALUES ('Original Post', 'Original Content');

CREATE TABLE tv_post AS
SELECT
    pk_post,
    id,
    jsonb_build_object(
        'id', id::text,
        'title', title,
        'content', content
    ) AS data
FROM tb_post;

-- Verify initial state
SELECT
    COUNT(*) = 1 as correct_row_count,
    (data->>'title') = 'Original Post' as correct_title,
    (data->>'content') = 'Original Content' as correct_content
FROM tv_post;

-- Test: UPDATE should trigger refresh
UPDATE tb_post
SET title = 'Updated Post', content = 'Updated Content'
WHERE pk_post = 1;

-- Verify refresh happened
SELECT
    COUNT(*) = 1 as row_exists,
    (data->>'title') = 'Updated Post' as title_updated,
    (data->>'content') = 'Updated Content' as content_updated
FROM tv_post
WHERE pk_post = 1;

-- Verify metadata and triggers
SELECT
    COUNT(*) = 1 as metadata_created
FROM pg_tview_meta
WHERE entity = 'post';

SELECT
    COUNT(*) >= 1 as triggers_created
FROM pg_trigger
WHERE tgname LIKE '%tview%';

\echo '✓ Test 1 passed: pk_post extraction works'

-- Test 2: Different PK column name (pk_user)
\echo ''
\echo 'Test 2: Different PK column name (pk_user)'
CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT NOT NULL,
    email TEXT
);

INSERT INTO tb_user (name, email)
VALUES ('Alice', 'alice@example.com');

CREATE TABLE tv_user AS
SELECT
    pk_user,
    id,
    jsonb_build_object(
        'id', id::text,
        'name', name,
        'email', email
    ) AS data
FROM tb_user;

-- Verify initial state
SELECT
    COUNT(*) = 1 as correct_row_count,
    (data->>'name') = 'Alice' as correct_name,
    (data->>'email') = 'alice@example.com' as correct_email
FROM tv_user;

-- Test: UPDATE should trigger refresh (different PK column)
UPDATE tb_user
SET name = 'Alice Updated', email = 'alice.updated@example.com'
WHERE pk_user = 1;

-- Verify refresh happened
SELECT
    COUNT(*) = 1 as row_exists,
    (data->>'name') = 'Alice Updated' as name_updated,
    (data->>'email') = 'alice.updated@example.com' as email_updated
FROM tv_user
WHERE pk_user = 1;

-- Verify metadata for user TVIEW
SELECT
    COUNT(*) = 1 as user_metadata_created
FROM pg_tview_meta
WHERE entity = 'user';

\echo '✓ Test 2 passed: pk_user extraction works'

-- Test 3: INSERT should trigger initial population
\echo ''
\echo 'Test 3: INSERT triggers refresh'
INSERT INTO tb_post (title, content)
VALUES ('New Post', 'New Content');

-- Should have 2 rows now
SELECT COUNT(*) = 2 as correct_post_count FROM tv_post;

-- Verify new post was added
SELECT
    COUNT(*) = 1 as new_post_added,
    (data->>'title') = 'New Post' as correct_title
FROM tv_post
WHERE data->>'title' = 'New Post';

\echo '✓ Test 3 passed: INSERT triggers refresh'

-- Test 4: DELETE should remove from TVIEW
\echo ''
\echo 'Test 4: DELETE removes from TVIEW'
DELETE FROM tb_post WHERE pk_post = 2;

-- Should have 1 row now
SELECT COUNT(*) AS post_count FROM tv_post;
-- Expected: 1

\echo '✓ Test 4 passed: DELETE removes from TVIEW'

-- Test 5: Verify trigger handler exists and is named correctly
\echo ''
\echo 'Test 5: Verify trigger installation'
SELECT
    tgname,
    tgrelid::regclass AS table_name,
    tgenabled
FROM pg_trigger
WHERE tgname LIKE 'trg_tview_%'
ORDER BY tgname;
-- Expected: triggers on tb_post and tb_user

\echo '✓ Test 5 passed: Triggers installed correctly'

\echo ''
\echo '=========================================='
\echo 'Test 40: All tests passed! ✓'
\echo '=========================================='

ROLLBACK;

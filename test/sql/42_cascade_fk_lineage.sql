-- Test 42: FK Lineage Cascade
-- Purpose: Verify cascade propagation through FK relationships
-- Expected: Update to parent entity cascades to all dependent child rows

\set ECHO all
\set ON_ERROR_STOP on

BEGIN;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;

CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\echo '=========================================='
\echo 'Test 42: FK Lineage Cascade'
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

-- Create helper views (workaround for parser)
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
        'author', user_prepared.data
    ) AS data
FROM tb_post p
JOIN user_prepared ON user_prepared.pk_user = p.fk_user;

-- Create TVIEWs using SQL functions (order matters: parent first)
SELECT pg_tviews_create('tv_user', 'SELECT pk_user, id, data FROM user_prepared');
SELECT pg_tviews_create('tv_post', 'SELECT pk_post, id, fk_user, user_id, data FROM post_prepared');

-- Test 1: Verify initial state
\echo ''
\echo 'Test 1: Verify initial population'
SELECT COUNT(*) = 2 as correct_user_count FROM tv_user;

SELECT COUNT(*) = 4 as correct_post_count FROM tv_post;

-- Verify nested author data
SELECT
    COUNT(*) = 4 as all_posts_have_authors,
    COUNT(*) FILTER (WHERE data->'author'->>'name' = 'Alice') = 3 as alice_has_3_posts,
    COUNT(*) FILTER (WHERE data->'author'->>'name' = 'Bob') = 1 as bob_has_1_post
FROM tv_post;
WHERE fk_user = 1
ORDER BY pk_post;
-- Expected: 3 posts with author 'Alice', 'alice@example.com'

\echo '✓ Test 1 passed: Initial population correct'

-- Test 2: Update parent (user) - should cascade to posts
\echo ''
\echo 'Test 2: Update parent cascades to children'

-- Update Alice's name
UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1;

-- Verify user updated
SELECT (data->>'name') = 'Alice Updated' as user_updated FROM tv_user WHERE pk_user = 1;

-- Verify ALL posts by Alice have updated author name
SELECT
    COUNT(*) = 3 as all_alice_posts_updated,
    COUNT(*) FILTER (WHERE data->'author'->>'name' = 'Alice Updated') = 3 as all_have_correct_name
FROM tv_post
WHERE fk_user = 1;

-- Verify Bob's posts NOT affected
SELECT
    COUNT(*) = 1 as bob_posts_unchanged,
    (data->'author'->>'name') = 'Bob' as bob_name_correct
FROM tv_post
WHERE fk_user = 2;

\echo '✓ Test 2 passed: Parent update cascaded to children'

-- Test 3: Update multiple fields in parent
\echo ''
\echo 'Test 3: Multiple field update cascades'

UPDATE tb_user
SET name = 'Alice V2', email = 'alice.v2@example.com'
WHERE pk_user = 1;

-- Verify cascade updated both fields
SELECT
    (data->'author'->>'name') = 'Alice V2' as name_updated,
    (data->'author'->>'email') = 'alice.v2@example.com' as email_updated
FROM tv_post
WHERE pk_post = 1;

\echo '✓ Test 3 passed: Multiple fields cascaded'

-- Test 4: Update child (post) - should NOT cascade to user
\echo ''
\echo 'Test 4: Child update does not cascade to parent'

-- Record user timestamp before post update
SELECT updated_at AS user_before FROM tv_user WHERE pk_user = 1 \gset

-- Wait briefly
SELECT pg_sleep(0.1);

-- Update post
UPDATE tb_post SET title = 'Alice Post 1 Updated' WHERE pk_post = 1;

-- Verify post updated
SELECT data->>'title' FROM tv_post WHERE pk_post = 1;
-- Expected: 'Alice Post 1 Updated'

-- Verify user NOT updated (timestamp unchanged)
SELECT updated_at = :'user_before'::timestamptz AS user_unchanged
FROM tv_user WHERE pk_user = 1;
-- Expected: true (user should not have been touched)

\echo '✓ Test 4 passed: Child update did not cascade to parent'

-- Test 5: Change FK relationship
\echo ''
\echo 'Test 5: FK change updates cascades correctly'

-- Move post from Alice to Bob
UPDATE tb_post SET fk_user = 2 WHERE pk_post = 1;

-- Verify post now has Bob as author
SELECT
    pk_post,
    data->>'title' AS title,
    data->'author'->>'name' AS author_name
FROM tv_post
WHERE pk_post = 1;
-- Expected: 'Bob'

-- Alice should now have only 2 posts
SELECT COUNT(*) FROM tv_post WHERE fk_user = 1;
-- Expected: 2

-- Bob should now have 2 posts
SELECT COUNT(*) FROM tv_post WHERE fk_user = 2;
-- Expected: 2

\echo '✓ Test 5 passed: FK change handled correctly'

-- Test 6: INSERT new child - should use parent data
\echo ''
\echo 'Test 6: INSERT new child populates from parent'

INSERT INTO tb_post (fk_user, title, content)
VALUES (1, 'New Alice Post', 'New content');

-- Should have author data from current Alice
SELECT
    data->>'title' AS title,
    data->'author'->>'name' AS author_name,
    data->'author'->>'email' AS author_email
FROM tv_post
WHERE data->>'title' = 'New Alice Post';
-- Expected: 'New Alice Post', 'Alice V2', 'alice.v2@example.com'

\echo '✓ Test 6 passed: INSERT uses current parent data'

-- Test 7: DELETE child - should not affect parent
\echo ''
\echo 'Test 7: DELETE child does not affect parent'

DELETE FROM tb_post WHERE pk_post = 2;

-- Verify post deleted from TVIEW
SELECT COUNT(*) FROM tv_post WHERE pk_post = 2;
-- Expected: 0

-- Verify user still exists
SELECT COUNT(*) FROM tv_user WHERE pk_user = 1;
-- Expected: 1

\echo '✓ Test 7 passed: Child deletion handled correctly'

-- Test 8: Verify dependency metadata
\echo ''
\echo 'Test 8: Verify dependency metadata'

SELECT
    entity,
    array_length(dependencies, 1) AS dependency_count,
    array_length(fk_columns, 1) AS fk_count
FROM pg_tview_meta
ORDER BY entity;
-- Expected: user (0 deps, 0 fks), post (depends on user, 1 fk)

\echo '✓ Test 8 passed: Metadata correct'

\echo ''
\echo '=========================================='
\echo 'Test 42: All tests passed! ✓'
\echo '=========================================='

ROLLBACK;

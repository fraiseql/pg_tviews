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
\ir lib/assert_fresh.sql

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
    user_prepared.id AS user_id,
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
SELECT assert_fresh('tv_user', 'pk_user', 'pg_tviews_create');
SELECT assert_fresh('tv_post', 'pk_post', 'pg_tviews_create');
DO $$ BEGIN
    IF (SELECT COUNT(*) FROM tv_user) <> 2 OR (SELECT COUNT(*) FROM tv_post) <> 4
       OR (SELECT COUNT(*) FROM tv_post WHERE data->'author'->>'name' = 'Alice') <> 3
       OR (SELECT COUNT(*) FROM tv_post WHERE data->'author'->>'name' = 'Bob') <> 1 THEN
        RAISE EXCEPTION 'FAIL: initial users/posts or nested authors wrong';
    END IF;
END $$;

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
    (SELECT COUNT(*) FROM tv_post WHERE fk_user = 2) = 1 as bob_posts_unchanged,
    (SELECT data->'author'->>'name' FROM tv_post WHERE fk_user = 2) = 'Bob' as bob_name_correct;

SELECT assert_fresh('tv_user', 'pk_user', 'an UPDATE of tb_user.name');
SELECT assert_fresh('tv_post', 'pk_post', 'an UPDATE of tb_user.name');
DO $$ BEGIN
    IF (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) IS DISTINCT FROM 'Alice Updated' THEN
        RAISE EXCEPTION 'FAIL: tv_user row 1 not refreshed';
    END IF;
    IF (SELECT COUNT(*) FROM tv_post WHERE fk_user = 1 AND data->'author'->>'name' = 'Alice Updated') <> 3 THEN
        RAISE EXCEPTION 'FAIL: user rename did not reach all 3 of her posts';
    END IF;
    IF (SELECT data->'author'->>'name' FROM tv_post WHERE fk_user = 2) IS DISTINCT FROM 'Bob' THEN
        RAISE EXCEPTION 'FAIL: Bob''s post changed by Alice''s rename';
    END IF;
END $$;

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

SELECT assert_fresh('tv_user', 'pk_user', 'an UPDATE of two tb_user fields');
SELECT assert_fresh('tv_post', 'pk_post', 'an UPDATE of two tb_user fields');
DO $$ BEGIN
    IF (SELECT (data->'author'->>'name', data->'author'->>'email') FROM tv_post WHERE pk_post = 1)
       IS DISTINCT FROM ('Alice V2'::text, 'alice.v2@example.com'::text) THEN
        RAISE EXCEPTION 'FAIL: post 1 author not (Alice V2, alice.v2@example.com)';
    END IF;
END $$;

\echo '✓ Test 3 passed: Multiple fields cascaded'

-- Test 4: Update child (post) - should NOT cascade to user
\echo ''
\echo 'Test 4: Child update does not cascade to parent'

-- Record the user row's tuple id before the post update: the file runs in one
-- transaction, where updated_at (now()) cannot change, but any rewrite of the
-- row gives it a new ctid.
CREATE TEMP TABLE user_before AS SELECT ctid AS tid FROM tv_user WHERE pk_user = 1;

-- Update post
UPDATE tb_post SET title = 'Alice Post 1 Updated' WHERE pk_post = 1;

-- Verify post updated
SELECT data->>'title' FROM tv_post WHERE pk_post = 1;
-- Expected: 'Alice Post 1 Updated'

-- Verify user NOT updated (row not rewritten)
SELECT ctid = (SELECT tid FROM user_before) AS user_unchanged
FROM tv_user WHERE pk_user = 1;

SELECT assert_fresh('tv_user', 'pk_user', 'an UPDATE of tb_post.title');
SELECT assert_fresh('tv_post', 'pk_post', 'an UPDATE of tb_post.title');
DO $$ BEGIN
    IF (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 'Alice Post 1 Updated' THEN
        RAISE EXCEPTION 'FAIL: post 1 title not refreshed';
    END IF;
    IF (SELECT ctid FROM tv_user WHERE pk_user = 1) IS DISTINCT FROM (SELECT tid FROM user_before) THEN
        RAISE EXCEPTION 'FAIL: a post update rewrote its parent user row';
    END IF;
END $$;

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

SELECT assert_fresh('tv_user', 'pk_user', 're-pointing tb_post.fk_user');
SELECT assert_fresh('tv_post', 'pk_post', 're-pointing tb_post.fk_user');
DO $$ BEGIN
    IF (SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 1) IS DISTINCT FROM 'Bob' THEN
        RAISE EXCEPTION 'FAIL: moved post 1 does not show Bob as author';
    END IF;
    IF (SELECT COUNT(*) FROM tv_post WHERE fk_user = 1) <> 2
       OR (SELECT COUNT(*) FROM tv_post WHERE fk_user = 2) <> 2 THEN
        RAISE EXCEPTION 'FAIL: post counts per user not 2/2 after the move';
    END IF;
END $$;

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

SELECT assert_fresh('tv_user', 'pk_user', 'an INSERT into tb_post');
SELECT assert_fresh('tv_post', 'pk_post', 'an INSERT into tb_post');
DO $$ BEGIN
    IF (SELECT (data->'author'->>'name', data->'author'->>'email') FROM tv_post
        WHERE data->>'title' = 'New Alice Post')
       IS DISTINCT FROM ('Alice V2'::text, 'alice.v2@example.com'::text) THEN
        RAISE EXCEPTION 'FAIL: new post does not embed the current Alice';
    END IF;
END $$;

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

SELECT assert_fresh('tv_user', 'pk_user', 'a DELETE from tb_post');
SELECT assert_fresh('tv_post', 'pk_post', 'a DELETE from tb_post');
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tv_post WHERE pk_post = 2) THEN
        RAISE EXCEPTION 'FAIL: deleted post 2 still in tv_post';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM tv_user WHERE pk_user = 1) THEN
        RAISE EXCEPTION 'FAIL: deleting a post removed its user';
    END IF;
END $$;

\echo '✓ Test 7 passed: Child deletion handled correctly'

-- Test 8: Verify dependency metadata
\echo ''
\echo 'Test 8: Verify dependency metadata'

SELECT
    entity,
    jsonb_array_length(plan->'paths') AS local_paths,
    jsonb_array_length(plan->'embeds') AS embeds
FROM pg_tview_meta
ORDER BY entity;
-- Expected: user (1 local path, 0 embeds), post (1 local path, 0 embeds: it reads tb_user)

-- post reads tb_user through a plain view, not through tv_user: that is a
-- mapped table in its plan (keyed back through fk_user), not an embed.
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_tview_meta
               WHERE jsonb_array_length(plan->'paths') <> 1
                  OR jsonb_array_length(plan->'embeds') <> 0) THEN
        RAISE EXCEPTION 'FAIL: expected 1 local path and 0 embeds per TVIEW';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_tview_meta m, jsonb_array_elements(m.plan->'tables') t
                   WHERE m.entity = 'post' AND t->>'kind' = 'mapped'
                     AND (t->>'relid')::oid = 'tb_user'::regclass::oid) THEN
        RAISE EXCEPTION 'FAIL: post plan does not map tb_user';
    END IF;
END $$;

\echo '✓ Test 8 passed: Metadata correct'

\echo ''
\echo '=========================================='
\echo 'Test 42: All tests passed! ✓'
\echo '=========================================='

ROLLBACK;

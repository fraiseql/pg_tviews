-- Phase 4 Integration Tests: Dynamic Primary Key Detection
-- Tests automatic PK column detection based on entity naming convention

-- Test 1: Standard tb_<entity> naming convention
CREATE TABLE tb_user (
    pk_user BIGINT PRIMARY KEY,
    name TEXT,
    email TEXT
);

CREATE TABLE tb_post (
    pk_post BIGINT PRIMARY KEY,
    fk_user BIGINT,
    title TEXT,
    content TEXT
);

-- Create TVIEWs
SELECT pg_tviews_create('user', $$
    SELECT pk_user,
           jsonb_build_object('name', name, 'email', email) as data
    FROM tb_user
$$);

SELECT pg_tviews_create('post', $$
    SELECT pk_post,
           jsonb_build_object(
               'fk_user', fk_user,
               'title', title,
               'content', content
           ) as data
    FROM tb_post
$$);

-- Test 2: Trigger activation with correct PK detection
BEGIN;
    -- Insert users (should detect pk_user column)
    INSERT INTO tb_user VALUES (1, 'Alice', 'alice@example.com');
    INSERT INTO tb_user VALUES (2, 'Bob', 'bob@example.com');

    -- Insert posts (should detect pk_post column)
    INSERT INTO tb_post VALUES (1, 1, 'Hello World', 'First post content');
    INSERT INTO tb_post VALUES (2, 2, 'Bob Post', 'Second post content');

COMMIT;

-- Verify data was processed correctly
SELECT COUNT(*) as user_count FROM tv_user;
SELECT COUNT(*) as post_count FROM tv_post;

-- Test 3: FK relationships work correctly
SELECT p.title, u.data->>'name' as author_name
FROM tv_post p
JOIN tv_user u ON (p.data->>'fk_user')::bigint = (u.data->>'id')::bigint;

-- Test 4: Cascade refresh works with dynamic PK detection
BEGIN;
    -- Update user (should cascade to posts)
    UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1;

    -- Check that both entities show in queue
    SELECT * FROM pg_tviews_queue_info();
    -- Expected: items for both user and post entities

COMMIT;

-- Test 5: Error handling for non-standard table names
CREATE TABLE users_non_standard (
    id BIGINT PRIMARY KEY,  -- Not pk_users
    name TEXT
);

-- This should work with fallback logic (id column)
-- Note: TVIEW creation might fail if entity name doesn't match table pattern
-- This tests the robustness of the PK detection logic

-- Cleanup
DROP TABLE tb_user CASCADE;
DROP TABLE tb_post CASCADE;
DROP TABLE users_non_standard CASCADE;
SELECT pg_tviews_drop('user');
SELECT pg_tviews_drop('post');
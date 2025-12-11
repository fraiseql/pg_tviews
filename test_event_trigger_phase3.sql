-- Test Event Trigger Phase 3
-- This test verifies that the ProcessUtility hook only validates
-- and the event trigger handles all TVIEW creation

\set ECHO all

-- Setup test data following trinity pattern
CREATE TABLE tb_user (
    id UUID DEFAULT gen_random_uuid(),
    pk_user SERIAL PRIMARY KEY,
    identifier TEXT UNIQUE,
    name TEXT NOT NULL,
    email TEXT UNIQUE
);

CREATE TABLE tb_post (
    id UUID DEFAULT gen_random_uuid(),
    pk_post SERIAL PRIMARY KEY,
    fk_user INTEGER REFERENCES tb_user(pk_user),
    identifier TEXT UNIQUE,
    title TEXT NOT NULL,
    content TEXT
);

-- Insert test data
INSERT INTO tb_user (identifier, name, email)
VALUES ('alice', 'Alice Smith', 'alice@example.com');

INSERT INTO tb_post (fk_user, identifier, title, content)
SELECT pk_user, 'hello-world', 'Hello World', 'My first post!'
FROM tb_user WHERE identifier = 'alice';

-- Test 1: Valid TVIEW syntax - hook should validate, event trigger should convert
CREATE TABLE tv_post AS
SELECT
    p.pk_post as pk_post,
    p.id as id,
    p.identifier as identifier,
    p.fk_user as fk_user,
    u.id as user_id,
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'author', jsonb_build_object(
            'id', u.id,
            'identifier', u.identifier,
            'name', u.name,
            'email', u.email
        )
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;

-- Check that TVIEW was created by event trigger
SELECT 'Event trigger created TVIEW' as test,
       EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'post') as passed;

SELECT 'Backing view exists' as test,
       EXISTS(SELECT 1 FROM pg_views WHERE viewname = 'v_post') as passed;

SELECT 'TVIEW wrapper exists' as test,
       EXISTS(SELECT 1 FROM pg_views WHERE viewname = 'tv_post') as passed;

-- Check data is accessible
SELECT 'Data accessible via TVIEW' as test, COUNT(*) = 1 as passed FROM tv_post;

-- Test 2: Invalid TVIEW syntax - hook should log warning, event trigger should detect invalid structure
CREATE TABLE tv_invalid AS
SELECT
    id as pk_invalid,  -- Missing proper pk_ prefix
    name as name       -- Missing jsonb_build_object for data
FROM tb_user;

-- Check that table exists but is not a proper TVIEW
SELECT 'Invalid table created' as test,
       EXISTS(SELECT 1 FROM information_schema.tables WHERE table_name = 'tv_invalid') as passed;

SELECT 'Not registered as TVIEW' as test,
       NOT EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'invalid') as passed;

-- Test 3: Non-TVIEW table - should pass through unchanged
CREATE TABLE regular_table (id INT, name TEXT);
INSERT INTO regular_table VALUES (1, 'test');

SELECT 'Regular table works' as test,
       (SELECT COUNT(*) FROM regular_table) = 1 as passed;

-- Cleanup
DROP TABLE tv_post CASCADE;
DROP TABLE tv_invalid;
DROP TABLE regular_table;
DROP TABLE tb_post;
DROP TABLE tb_user;

SELECT 'Phase 3 test complete' as status;
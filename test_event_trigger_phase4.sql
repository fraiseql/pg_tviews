-- Test Event Trigger Phase 4 - Edge Cases
-- This test verifies edge case handling: empty tables, hints, rollback, inference

\set ECHO all

-- Test 1: Empty table handling
CREATE TABLE tb_empty (
    id UUID DEFAULT gen_random_uuid(),
    pk_empty SERIAL PRIMARY KEY,
    name TEXT
);

-- Create empty TVIEW
CREATE TABLE tv_empty AS
SELECT
    pk_empty as pk_empty,
    id as id,
    jsonb_build_object('name', name) as data
FROM tb_empty;

-- Check that empty TVIEW was created properly
SELECT 'Empty TVIEW created' as test,
       EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'empty') as passed;

SELECT 'Empty view exists' as test,
       EXISTS(SELECT 1 FROM pg_views WHERE viewname = 'v_empty') as passed;

SELECT 'Empty TVIEW wrapper exists' as test,
       EXISTS(SELECT 1 FROM pg_views WHERE viewname = 'tv_empty') as passed;

-- Check that empty TVIEW returns no rows
SELECT 'Empty TVIEW returns no rows' as test, (SELECT COUNT(*) FROM tv_empty) = 0 as passed;

-- Test 2: HINT system for complex SELECTs
CREATE TABLE tb_user (
    id UUID DEFAULT gen_random_uuid(),
    pk_user SERIAL PRIMARY KEY,
    name TEXT
);

CREATE TABLE tb_post (
    id UUID DEFAULT gen_random_uuid(),
    pk_post SERIAL PRIMARY KEY,
    fk_user INTEGER REFERENCES tb_user(pk_user),
    title TEXT
);

INSERT INTO tb_user (name) VALUES ('Alice');
INSERT INTO tb_post (fk_user, title) SELECT pk_user, 'Hello' FROM tb_user WHERE name = 'Alice';

-- Create TVIEW with hints for base tables
CREATE TABLE tv_post_hint AS
SELECT
    p.pk_post as pk_post,
    p.id as id,
    jsonb_build_object(
        'title', p.title,
        'author', jsonb_build_object('id', u.id, 'name', u.name)
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;

-- Add hint comment for base tables
COMMENT ON TABLE tv_post_hint IS 'TVIEW_BASES: tb_post, tb_user';

-- The event trigger should detect the hint and use it for trigger installation
SELECT 'Hinted TVIEW created' as test,
       EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'post_hint') as passed;

-- Test 3: Base table inference from data
CREATE TABLE tv_inference AS
SELECT
    p.pk_post as pk_post,
    p.id as id,
    jsonb_build_object(
        'title', p.title,
        'fk_user', p.fk_user,
        'user_id', u.id
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;

-- Event trigger should try to infer tb_post and tb_user from the data patterns
SELECT 'Inference TVIEW created' as test,
       EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'inference') as passed;

-- Test 4: Invalid structure (should still create table but log warnings)
CREATE TABLE tv_invalid AS
SELECT
    id as pk_invalid,  -- Missing proper pk_ prefix
    name as name       -- Missing jsonb_build_object
FROM tb_user;

-- Table should exist but not be a proper TVIEW
SELECT 'Invalid table created' as test,
       EXISTS(SELECT 1 FROM information_schema.tables WHERE table_name = 'tv_invalid') as passed;

SELECT 'Not registered as TVIEW' as test,
       NOT EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'invalid') as passed;

-- Test 5: Complex JOIN scenario
CREATE TABLE tb_category (
    id UUID DEFAULT gen_random_uuid(),
    pk_category SERIAL PRIMARY KEY,
    name TEXT
);

INSERT INTO tb_category (name) VALUES ('Tech');

UPDATE tb_post SET fk_user = (SELECT pk_user FROM tb_user WHERE name = 'Alice') WHERE title = 'Hello';

CREATE TABLE tv_complex AS
SELECT
    p.pk_post as pk_post,
    p.id as id,
    jsonb_build_object(
        'title', p.title,
        'author', jsonb_build_object('name', u.name),
        'category', jsonb_build_object('name', c.name)
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user
JOIN tb_category c ON c.name = 'Tech';  -- Cross join for testing

-- Add hints for complex scenario
COMMENT ON TABLE tv_complex IS 'TVIEW_BASES: tb_post, tb_user, tb_category';

SELECT 'Complex TVIEW created' as test,
       EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'complex') as passed;

-- Cleanup
DROP TABLE tv_empty CASCADE;
DROP TABLE tv_post_hint CASCADE;
DROP TABLE tv_inference CASCADE;
DROP TABLE tv_invalid;
DROP TABLE tv_complex CASCADE;
DROP TABLE tb_empty;
DROP TABLE tb_post;
DROP TABLE tb_user;
DROP TABLE tb_category;

SELECT 'Phase 4 edge cases test complete' as status;
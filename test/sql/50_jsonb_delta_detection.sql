-- Phase 5 Task 1 RED: Test jsonb_delta detection
-- This test verifies runtime detection of jsonb_delta extension

BEGIN;
    SET client_min_messages TO WARNING;

    -- Test Case 1: Detection when jsonb_delta NOT installed
    DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;

    -- Create pg_tviews without jsonb_delta
    CREATE EXTENSION pg_tviews;

    -- Should detect absence of jsonb_delta
    SELECT pg_tviews_check_jsonb_delta() AS jsonb_delta_available;
    -- Expected: f (false)

    -- Verify pg_tviews still works without jsonb_delta
    CREATE TABLE tb_test (pk_test INT PRIMARY KEY, id UUID, name TEXT);
    INSERT INTO tb_test VALUES (1, gen_random_uuid(), 'Test');

    SELECT pg_tviews_create('test', $$
        SELECT pk_test, id,
               jsonb_build_object('id', id, 'name', name) AS data
        FROM tb_test
    $$);

    -- Verify TVIEW created successfully
    SELECT COUNT(*) = 1 AS tview_created FROM pg_tview_meta WHERE entity = 'test';
    -- Expected: t

    -- Verify data populated correctly
    SELECT data->>'name' AS name FROM tv_test WHERE pk_test = 1;
    -- Expected: 'Test'

    -- Cleanup for next test case
    DROP TABLE IF EXISTS tb_test CASCADE;
    SELECT pg_tviews_drop('test');

    -- Test Case 2: Detection when jsonb_delta IS installed
    CREATE EXTENSION IF NOT EXISTS jsonb_delta;

    -- Should detect presence of jsonb_delta
    SELECT pg_tviews_check_jsonb_delta() AS jsonb_delta_available;
    -- Expected: t (true)

    -- Verify pg_tviews still works with jsonb_delta
    CREATE TABLE tb_test2 (pk_test2 INT PRIMARY KEY, id UUID, title TEXT);
    INSERT INTO tb_test2 VALUES (1, gen_random_uuid(), 'Test 2');

    SELECT pg_tviews_create('test2', $$
        SELECT pk_test2, id,
               jsonb_build_object('id', id, 'title', title) AS data
        FROM tb_test2
    $$);

    -- Verify TVIEW created successfully
    SELECT COUNT(*) = 1 AS tview_created FROM pg_tview_meta WHERE entity = 'test2';
    -- Expected: t

    -- Verify data populated correctly
    SELECT data->>'title' AS title FROM tv_test2 WHERE pk_test2 = 1;
    -- Expected: 'Test 2'

ROLLBACK;

-- This test verifies runtime detection of jsonb_delta extension

\set ON_ERROR_STOP on

BEGIN;
    SET client_min_messages TO WARNING;

    -- Test Case 1: Detection when jsonb_delta NOT installed
    DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;

    -- Create pg_tviews without jsonb_delta
    CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

    -- Should detect absence of jsonb_delta
    SELECT pg_tviews_check_jsonb_delta() AS jsonb_delta_available;
    -- Expected: f (false)
    DO $$ BEGIN
        IF pg_tviews_check_jsonb_delta() IS DISTINCT FROM false THEN
            RAISE EXCEPTION 'FAIL: jsonb_delta reported available after DROP EXTENSION';
        END IF;
    END $$;

    -- Verify pg_tviews still works without jsonb_delta
    CREATE TABLE tb_test (pk_test INT PRIMARY KEY, id UUID, name TEXT);
    INSERT INTO tb_test VALUES (1, gen_random_uuid(), 'Test');

    SELECT pg_tviews_create('test', $$
        SELECT pk_test, id,
               jsonb_build_object('id', id, 'name', name) AS data
        FROM tb_test
    $$);
    SELECT assert_fresh('tv_test', 'pk_test', 'pg_tviews_create without jsonb_delta');

    -- Verify TVIEW created successfully
    SELECT COUNT(*) = 1 AS tview_created FROM pg_tview_meta WHERE entity = 'test';
    -- Expected: t
    DO $$ BEGIN
        IF (SELECT COUNT(*) FROM pg_tview_meta WHERE entity = 'test') <> 1 THEN
            RAISE EXCEPTION 'FAIL: no pg_tview_meta row for test';
        END IF;
    END $$;

    -- Verify data populated correctly
    SELECT data->>'name' AS name FROM tv_test WHERE pk_test = 1;
    -- Expected: 'Test'
    DO $$ BEGIN
        IF (SELECT data->>'name' FROM tv_test WHERE pk_test = 1) IS DISTINCT FROM 'Test' THEN
            RAISE EXCEPTION 'FAIL: tv_test row 1 name is not Test';
        END IF;
    END $$;

    -- Writes must still refresh the TVIEW when jsonb_delta is absent.
    UPDATE tb_test SET name = 'Test renamed' WHERE pk_test = 1;
    SELECT assert_fresh('tv_test', 'pk_test', 'UPDATE tb_test without jsonb_delta');
    DO $$ BEGIN
        IF (SELECT data->>'name' FROM tv_test WHERE pk_test = 1) IS DISTINCT FROM 'Test renamed' THEN
            RAISE EXCEPTION 'FAIL: tv_test row 1 not refreshed by UPDATE tb_test';
        END IF;
    END $$;

    -- Cleanup for next test case. Dropping the base table now auto-deregisters
    -- the tview via the sql_drop event trigger (issue #53), so the explicit
    -- drop must tolerate an already-removed tview (if_exists = true).
    DROP TABLE IF EXISTS tb_test CASCADE;
    SELECT pg_tviews_drop('test', true);
    DO $$ BEGIN
        IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'test')
           OR to_regclass('tv_test') IS NOT NULL THEN
            RAISE EXCEPTION 'FAIL: TVIEW test survived DROP TABLE tb_test CASCADE';
        END IF;
    END $$;

    -- Test Case 2: Detection when jsonb_delta IS installed
    CREATE EXTENSION IF NOT EXISTS jsonb_delta;

    -- Should detect presence of jsonb_delta
    SELECT pg_tviews_check_jsonb_delta() AS jsonb_delta_available;
    -- Expected: t (true)
    DO $$ BEGIN
        IF pg_tviews_check_jsonb_delta() IS DISTINCT FROM true THEN
            RAISE EXCEPTION 'FAIL: jsonb_delta not detected after CREATE EXTENSION';
        END IF;
    END $$;

    -- Verify pg_tviews still works with jsonb_delta
    CREATE TABLE tb_test2 (pk_test2 INT PRIMARY KEY, id UUID, title TEXT);
    INSERT INTO tb_test2 VALUES (1, gen_random_uuid(), 'Test 2');

    SELECT pg_tviews_create('test2', $$
        SELECT pk_test2, id,
               jsonb_build_object('id', id, 'title', title) AS data
        FROM tb_test2
    $$);
    SELECT assert_fresh('tv_test2', 'pk_test2', 'pg_tviews_create with jsonb_delta');

    -- Verify TVIEW created successfully
    SELECT COUNT(*) = 1 AS tview_created FROM pg_tview_meta WHERE entity = 'test2';
    -- Expected: t
    DO $$ BEGIN
        IF (SELECT COUNT(*) FROM pg_tview_meta WHERE entity = 'test2') <> 1 THEN
            RAISE EXCEPTION 'FAIL: no pg_tview_meta row for test2';
        END IF;
    END $$;

    -- Verify data populated correctly
    SELECT data->>'title' AS title FROM tv_test2 WHERE pk_test2 = 1;
    -- Expected: 'Test 2'
    DO $$ BEGIN
        IF (SELECT data->>'title' FROM tv_test2 WHERE pk_test2 = 1) IS DISTINCT FROM 'Test 2' THEN
            RAISE EXCEPTION 'FAIL: tv_test2 row 1 title is not Test 2';
        END IF;
    END $$;

    -- The same write must refresh the TVIEW with jsonb_delta present.
    UPDATE tb_test2 SET title = 'Test 2 renamed' WHERE pk_test2 = 1;
    SELECT assert_fresh('tv_test2', 'pk_test2', 'UPDATE tb_test2 with jsonb_delta');
    DO $$ BEGIN
        IF (SELECT data->>'title' FROM tv_test2 WHERE pk_test2 = 1) IS DISTINCT FROM 'Test 2 renamed' THEN
            RAISE EXCEPTION 'FAIL: tv_test2 row 1 not refreshed by UPDATE tb_test2';
        END IF;
    END $$;

ROLLBACK;

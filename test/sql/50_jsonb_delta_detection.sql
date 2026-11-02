-- pg_tviews requires jsonb_delta: CREATE EXTENSION ... CASCADE installs it, and
-- TVIEWs refresh with the patch path off (recompute) and on.

\set ON_ERROR_STOP on

BEGIN;
    SET client_min_messages TO WARNING;

    -- Test Case 1: CASCADE installs the required jsonb_delta
    DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;

    CREATE EXTENSION pg_tviews CASCADE;
\ir lib/assert_fresh.sql

    DO $$ BEGIN
        IF NOT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'jsonb_delta') THEN
            RAISE EXCEPTION 'FAIL: CREATE EXTENSION pg_tviews CASCADE did not install jsonb_delta';
        END IF;
    END $$;

    -- Verify pg_tviews works with the patch path off (every write recomputes)
    SET LOCAL pg_tviews.direct_patch_enabled = off;
    CREATE TABLE tb_test (pk_test INT PRIMARY KEY, id UUID, name TEXT);
    INSERT INTO tb_test VALUES (1, gen_random_uuid(), 'Test');

    SELECT pg_tviews_create('test', $$
        SELECT pk_test, id,
               jsonb_build_object('id', id, 'name', name) AS data
        FROM tb_test
    $$);
    SELECT assert_fresh('tv_test', 'pk_test', 'pg_tviews_create, recompute');

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

    -- Writes refresh the TVIEW by recompute.
    UPDATE tb_test SET name = 'Test renamed' WHERE pk_test = 1;
    SELECT assert_fresh('tv_test', 'pk_test', 'UPDATE tb_test, recompute');
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

    -- Test Case 2: the patch path on
    SET LOCAL pg_tviews.direct_patch_enabled = on;

    -- Verify pg_tviews works with patches
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

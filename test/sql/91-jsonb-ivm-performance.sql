-- Standalone preamble (issue #55): make this file self-contained so it passes
-- on its own in a fresh database under psql -v ON_ERROR_STOP=1.
\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql
-- Test jsonb_delta performance impact
-- Compare TVIEW update performance with and without jsonb_delta

-- Clean up
DROP TABLE IF EXISTS tb_perf_test CASCADE;
DROP VIEW IF EXISTS v_perf_test CASCADE;
DROP TABLE IF EXISTS tv_perf_test CASCADE;

-- Create test table with JSONB data
CREATE TABLE tb_perf_test (
    pk_perf_test BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    data JSONB
);

-- Insert test data (1000 rows with nested JSONB)
INSERT INTO tb_perf_test (data)
SELECT jsonb_build_object(
    'field1', 'value_' || i,
    'field2', jsonb_build_object(
        'nested1', 'nested_value_' || i,
        'nested2', i,
        'array_field', jsonb_build_array('item1', 'item2', i)
    ),
    'field3', 'another_value_' || i
)
FROM generate_series(1, 1000) i;

-- Create TVIEW
SELECT pg_tviews_create('tv_perf_test', '
SELECT
    pk_perf_test,
    id,
    data
FROM tb_perf_test
');
SELECT assert_fresh('tv_perf_test', 'pk_perf_test', 'pg_tviews_create');
DO $$ BEGIN
    IF (SELECT count(*) FROM tv_perf_test) <> 1000 THEN
        RAISE EXCEPTION 'FAIL: tv_perf_test does not hold the 1000 seeded rows';
    END IF;
END $$;

-- Check current jsonb_delta status
SELECT 'Current jsonb_delta status:' as status;
SELECT pg_tviews_check_jsonb_delta();
DO $$ BEGIN
    IF pg_tviews_check_jsonb_delta() IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'FAIL: jsonb_delta not detected; this file measures it enabled';
    END IF;
END $$;

-- Test update performance (measure time for 100 updates)
SELECT 'Testing update performance...' as test;

-- Create a function to measure update time
CREATE OR REPLACE FUNCTION test_update_performance(iterations INT DEFAULT 100)
RETURNS TABLE (test_name TEXT, avg_time_ms FLOAT, total_time_ms FLOAT) AS $$
DECLARE
    start_time TIMESTAMPTZ;
    end_time TIMESTAMPTZ;
    i INT;
    total_time FLOAT := 0;
BEGIN
    -- Test updates
    FOR i IN 1..iterations LOOP
        start_time := clock_timestamp();
        
        -- Update a nested field (this should benefit from jsonb_delta)
        UPDATE tb_perf_test 
        SET data = jsonb_set(data, '{field2,nested1}'::text[], ('"updated_' || i || '"')::jsonb)
        WHERE pk_perf_test = i;
        
        end_time := clock_timestamp();
        total_time := total_time + extract(epoch from (end_time - start_time)) * 1000;
    END LOOP;
    
    RETURN QUERY SELECT 
        'jsonb_delta_' || CASE WHEN pg_tviews_check_jsonb_delta() THEN 'enabled' ELSE 'disabled' END,
        total_time / iterations,
        total_time;
END;
$$ LANGUAGE plpgsql;

-- Run performance test. The timings are information only (the host may be
-- noisy); what must hold is that every timed UPDATE refreshed the TVIEW.
SELECT * FROM test_update_performance(50);
SELECT assert_fresh('tv_perf_test', 'pk_perf_test', 'the 50 timed UPDATEs');
DO $$ BEGIN
    IF (SELECT count(*) FROM tv_perf_test) <> 1000
       OR (SELECT count(*) FROM tv_perf_test
           WHERE data #>> '{field2,nested1}' = 'updated_' || pk_perf_test) <> 50
       OR (SELECT data #>> '{field2,nested1}' FROM tv_perf_test WHERE pk_perf_test = 51)
          IS DISTINCT FROM 'nested_value_51' THEN
        RAISE EXCEPTION 'FAIL: tv_perf_test does not show exactly rows 1-50 updated';
    END IF;
END $$;

-- Clean up
DROP TABLE IF EXISTS tb_perf_test CASCADE;
DROP VIEW IF EXISTS v_perf_test CASCADE;
DROP TABLE IF EXISTS tv_perf_test CASCADE;
DROP FUNCTION IF EXISTS test_update_performance(INT);

-- Dropping the base table CASCADE removes the TVIEW and its registration.
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_tview_meta) OR to_regclass('tv_perf_test') IS NOT NULL THEN
        RAISE EXCEPTION 'FAIL: tv_perf_test survived the cleanup';
    END IF;
END $$;

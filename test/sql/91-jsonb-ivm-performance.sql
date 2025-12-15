-- Test jsonb_ivm performance impact
-- Compare TVIEW update performance with and without jsonb_ivm

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

-- Check current jsonb_ivm status
SELECT 'Current jsonb_ivm status:' as status;
SELECT pg_tviews_check_jsonb_ivm();

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
        
        -- Update a nested field (this should benefit from jsonb_ivm)
        UPDATE tb_perf_test 
        SET data = jsonb_set(data, '{field2,nested1}', '"updated_' || i || '"')
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

-- Run performance test
SELECT * FROM test_update_performance(50);

-- Clean up
DROP TABLE IF EXISTS tb_perf_test CASCADE;
DROP VIEW IF EXISTS v_perf_test CASCADE;
DROP TABLE IF EXISTS tv_perf_test CASCADE;
DROP FUNCTION IF EXISTS test_update_performance(INT);

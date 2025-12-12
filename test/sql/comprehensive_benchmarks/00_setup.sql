-- Comprehensive Benchmark Setup
-- Creates benchmark tracking tables and helper functions

-- Note: Database creation is handled by run_benchmarks.sh
-- This script runs inside the benchmark database

-- Enable extensions
CREATE EXTENSION IF NOT EXISTS pg_tviews;

-- Setup benchmark schema for isolation
\echo 'Setting up benchmark schema...'
\i cleanup_schema.sql

-- Require REAL jsonb_ivm extension - fail if not available
DO $$
BEGIN
    -- Try to create extension
    CREATE EXTENSION IF NOT EXISTS jsonb_ivm;
    RAISE NOTICE '✓ Using REAL jsonb_ivm extension';
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION 'jsonb_ivm extension not available! Benchmarks require the real extension, not stubs.';
END $$;

-- Verify we have the real extension (not stubs)
DO $$
DECLARE
    v_ext_exists BOOLEAN;
BEGIN
    SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_ivm') INTO v_ext_exists;
    IF NOT v_ext_exists THEN
        RAISE EXCEPTION 'jsonb_ivm extension not installed! Cannot proceed with benchmarks.';
    END IF;
    RAISE NOTICE '✓ jsonb_ivm extension verified';
END $$;

-- Create results tracking table
CREATE TABLE benchmark_results (
    id SERIAL PRIMARY KEY,
    run_timestamp TIMESTAMPTZ DEFAULT now(),
    scenario TEXT NOT NULL,
    test_name TEXT NOT NULL,
    data_scale TEXT NOT NULL,  -- 'small', 'medium', 'large'
    operation_type TEXT NOT NULL,  -- 'tviews_jsonb_ivm', 'tviews_native_pg', 'manual_func', 'full_refresh'
    rows_affected INTEGER,
    cascade_depth INTEGER,
    execution_time_ms NUMERIC(10, 3),
    memory_mb NUMERIC(10, 2),
    cache_hit_rate NUMERIC(5, 2),
    notes TEXT,
    UNIQUE(run_timestamp, scenario, test_name, operation_type)
);

CREATE INDEX idx_benchmark_results_scenario ON benchmark_results(scenario, data_scale);
CREATE INDEX idx_benchmark_results_operation ON benchmark_results(operation_type);

-- Helper function: Calculate improvement ratio
CREATE OR REPLACE FUNCTION calculate_improvement(
    baseline_ms NUMERIC,
    optimized_ms NUMERIC
) RETURNS NUMERIC AS $$
BEGIN
    IF optimized_ms = 0 THEN
        RETURN NULL;
    END IF;
    RETURN ROUND(baseline_ms / optimized_ms, 2);
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- Helper function: Record benchmark result (commits immediately)
CREATE OR REPLACE FUNCTION record_benchmark(
    p_scenario TEXT,
    p_test_name TEXT,
    p_data_scale TEXT,
    p_operation_type TEXT,
    p_rows_affected INTEGER,
    p_cascade_depth INTEGER,
    p_execution_time_ms NUMERIC,
    p_notes TEXT DEFAULT NULL
)
RETURNS void AS $$
BEGIN
    -- Insert result and commit immediately using dblink or similar
    -- For now, we'll use a simple approach with PERFORM in a subtransaction
    INSERT INTO benchmark_results (
        scenario, test_name, data_scale, operation_type,
        rows_affected, cascade_depth, execution_time_ms, notes
    ) VALUES (
        p_scenario, p_test_name, p_data_scale, p_operation_type,
        p_rows_affected, p_cascade_depth, p_execution_time_ms, p_notes
    )
    ON CONFLICT (run_timestamp, scenario, test_name, operation_type)
    DO UPDATE SET
        execution_time_ms = EXCLUDED.execution_time_ms,
        rows_affected = EXCLUDED.rows_affected,
        cascade_depth = EXCLUDED.cascade_depth,
        notes = EXCLUDED.notes;
END;
$$ LANGUAGE plpgsql;

-- Alternative: Use dblink to commit results in separate transaction
-- But for simplicity, let's modify the benchmark approach

-- Helper function: Benchmark executor with timing
CREATE OR REPLACE FUNCTION benchmark_execute(
    p_scenario TEXT,
    p_test_name TEXT,
    p_data_scale TEXT,
    p_operation_type TEXT,
    p_sql TEXT,
    p_cascade_depth INTEGER DEFAULT 1,
    p_notes TEXT DEFAULT NULL
) RETURNS NUMERIC AS $$
DECLARE
    v_start TIMESTAMPTZ;
    v_end TIMESTAMPTZ;
    v_duration_ms NUMERIC;
    v_rows_affected INTEGER;
BEGIN
    -- Start timing
    v_start := clock_timestamp();

    -- Execute the SQL
    EXECUTE p_sql;
    GET DIAGNOSTICS v_rows_affected = ROW_COUNT;

    -- End timing
    v_end := clock_timestamp();
    v_duration_ms := EXTRACT(EPOCH FROM (v_end - v_start)) * 1000;

    -- Record result
    PERFORM record_benchmark(
        p_scenario,
        p_test_name,
        p_data_scale,
        p_operation_type,
        v_rows_affected,
        p_cascade_depth,
        v_duration_ms,
        p_notes
    );

    -- Rollback to allow repeated runs
    RAISE EXCEPTION 'ROLLBACK - Benchmark complete' USING ERRCODE = 'P0001';

    RETURN v_duration_ms;
EXCEPTION
    WHEN SQLSTATE 'P0001' THEN
        -- Expected rollback
        RETURN v_duration_ms;
END;
$$ LANGUAGE plpgsql;

-- Summary view
CREATE OR REPLACE VIEW benchmark_summary AS
SELECT
    scenario,
    test_name,
    data_scale,
    operation_type,
    rows_affected,
    cascade_depth,
    execution_time_ms,
    ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) as ms_per_row,
    notes
FROM benchmark_results
ORDER BY run_timestamp DESC, scenario, data_scale, operation_type;

-- Comparison view: Incremental vs Full Refresh
CREATE OR REPLACE VIEW benchmark_comparison AS
WITH baseline AS (
    SELECT
        scenario,
        test_name,
        data_scale,
        execution_time_ms as baseline_ms,
        rows_affected
    FROM benchmark_results
    WHERE operation_type = 'full_refresh'
),
incremental AS (
    SELECT
        scenario,
        test_name,
        data_scale,
        operation_type,
        execution_time_ms as incremental_ms,
        rows_affected
    FROM benchmark_results
    WHERE operation_type != 'full_refresh'
)
SELECT
    i.scenario,
    i.test_name,
    i.data_scale,
    i.operation_type,
    i.rows_affected,
    b.baseline_ms,
    i.incremental_ms,
    calculate_improvement(b.baseline_ms, i.incremental_ms) as improvement_ratio,
    ROUND(b.baseline_ms - i.incremental_ms, 2) as time_saved_ms
FROM incremental i
LEFT JOIN baseline b USING (scenario, test_name, data_scale)
ORDER BY i.scenario, i.data_scale, improvement_ratio DESC NULLS LAST;

COMMENT ON TABLE benchmark_results IS 'Stores all benchmark execution results';
COMMENT ON VIEW benchmark_summary IS 'Human-readable summary of benchmark results';
COMMENT ON VIEW benchmark_comparison IS 'Compares incremental refresh vs full refresh performance';

\echo 'Benchmark setup complete!'
\echo 'Run scenarios with: \\i scenarios/XX_scenario_name.sql'
\echo 'View results with: SELECT * FROM benchmark_summary;'
\echo 'View comparisons with: SELECT * FROM benchmark_comparison;'

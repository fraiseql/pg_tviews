-- Single row overhead test: Measure actual database update overhead
-- Tests the real cost of smart patching vs full replacement on single rows

\timing on

DO $$
DECLARE
    start_time timestamptz;
    end_time timestamptz;
    duration_ms numeric;
    patch jsonb;
BEGIN
    RAISE NOTICE 'Testing single row update overhead...';

    -- Test 1: Full document replacement (baseline)
    start_time := clock_timestamp();

    UPDATE bench_authors
    SET name = 'Full Replace Author',
        email = 'full@example.com'
    WHERE id = 100;

    UPDATE tv_bench_posts tp
    SET data = vp.data
    FROM v_bench_posts vp
    WHERE tp.id = vp.id AND tp.author_id = 100;

    end_time := clock_timestamp();
    duration_ms := EXTRACT(EPOCH FROM (end_time - start_time)) * 1000;

    RAISE NOTICE 'Single row FULL REPLACEMENT: %.4f ms', duration_ms;

    -- Reset data
    RAISE EXCEPTION 'ROLLBACK' USING ERRCODE = 'P0001';
END $$;

\timing off
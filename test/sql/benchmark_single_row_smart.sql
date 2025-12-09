-- Single row smart patch test: Compare with smart patching

\timing on

DO $$
DECLARE
    start_time timestamptz;
    end_time timestamptz;
    duration_ms numeric;
    patch jsonb;
BEGIN
    RAISE NOTICE 'Testing single row smart patch overhead...';

    -- Test: Smart patching
    start_time := clock_timestamp();

    UPDATE bench_authors
    SET name = 'Smart Patch Author',
        email = 'smart@example.com'
    WHERE id = 100;

    -- Build patch
    SELECT jsonb_build_object('id', id, 'name', name, 'email', email)
    INTO patch FROM bench_authors WHERE id = 100;

    -- Smart patch update
    UPDATE tv_bench_posts
    SET data = jsonb_smart_patch_nested(data, patch, ARRAY['author'])
    WHERE author_id = 100;

    end_time := clock_timestamp();
    duration_ms := EXTRACT(EPOCH FROM (end_time - start_time)) * 1000;

    RAISE NOTICE 'Single row SMART PATCH: %.4f ms', duration_ms;

    -- Reset data
    RAISE EXCEPTION 'ROLLBACK' USING ERRCODE = 'P0001';
END $$;

\timing off
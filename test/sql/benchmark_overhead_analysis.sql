-- Overhead analysis: Test smart patching overhead on single row updates
-- This isolates the function call overhead without cascade complexity

\timing on

DO $$
DECLARE
    start_time timestamptz;
    end_time timestamptz;
    duration_ms numeric;
    test_data jsonb;
    patch jsonb;
    iterations int := 1000; -- Run many iterations to measure overhead
    i int;
BEGIN
    -- Create test data similar to our TVIEW format
    test_data := jsonb_build_object(
        'id', 1,
        'title', 'Test Post',
        'content', 'Test content',
        'author', jsonb_build_object(
            'id', 1,
            'name', 'Test Author',
            'email', 'test@example.com'
        ),
        'comments', jsonb_build_array(
            jsonb_build_object(
                'id', 1,
                'content', 'Test comment',
                'author', jsonb_build_object(
                    'id', 1,
                    'name', 'Test Author',
                    'email', 'test@example.com'
                )
            )
        )
    );

    -- Create patch for author update
    patch := jsonb_build_object(
        'id', 1,
        'name', 'Updated Author',
        'email', 'updated@example.com'
    );

    RAISE NOTICE 'Testing smart patch overhead with % iterations', iterations;

    -- Test 1: Measure overhead of jsonb_smart_patch_nested
    start_time := clock_timestamp();
    FOR i IN 1..iterations LOOP
        PERFORM jsonb_smart_patch_nested(test_data, patch, ARRAY['author']);
    END LOOP;
    end_time := clock_timestamp();
    duration_ms := EXTRACT(EPOCH FROM (end_time - start_time)) * 1000;

    RAISE NOTICE 'jsonb_smart_patch_nested overhead: %.4f ms per call (%.2f μs)', duration_ms/iterations, (duration_ms/iterations)*1000;

    -- Test 2: Measure overhead of jsonb_smart_patch_array
    start_time := clock_timestamp();
    FOR i IN 1..iterations LOOP
        PERFORM jsonb_smart_patch_array(test_data, jsonb_build_object('id', 1, 'author', patch), ARRAY['comments'], 'id');
    END LOOP;
    end_time := clock_timestamp();
    duration_ms := EXTRACT(EPOCH FROM (end_time - start_time)) * 1000;

    RAISE NOTICE 'jsonb_smart_patch_array overhead: %.4f ms per call (%.2f μs)', duration_ms/iterations, (duration_ms/iterations)*1000;

    -- Test 3: Compare with simple JSONB operations
    start_time := clock_timestamp();
    FOR i IN 1..iterations LOOP
        PERFORM test_data || jsonb_build_object('author', patch);
    END LOOP;
    end_time := clock_timestamp();
    duration_ms := EXTRACT(EPOCH FROM (end_time - start_time)) * 1000;

    RAISE NOTICE 'Simple || operator: %.4f ms per call (%.2f μs)', duration_ms/iterations, (duration_ms/iterations)*1000;

    -- Test 4: Measure full document replacement overhead
    start_time := clock_timestamp();
    FOR i IN 1..iterations LOOP
        PERFORM jsonb_build_object(
            'id', 1,
            'title', 'Test Post',
            'content', 'Test content',
            'author', patch,
            'comments', jsonb_build_array(
                jsonb_build_object(
                    'id', 1,
                    'content', 'Test comment',
                    'author', patch
                )
            )
        );
    END LOOP;
    end_time := clock_timestamp();
    duration_ms := EXTRACT(EPOCH FROM (end_time - start_time)) * 1000;

    RAISE NOTICE 'Full document rebuild: %.4f ms per call (%.2f μs)', duration_ms/iterations, (duration_ms/iterations)*1000;

    RAISE NOTICE 'OVERHEAD ANALYSIS COMPLETE - No rollback needed for this test';
END $$;

\timing off
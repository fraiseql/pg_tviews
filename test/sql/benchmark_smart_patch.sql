-- Smart patching benchmark: Uses jsonb_smart_patch_* functions
-- This simulates the NEW behavior with smart patching

\timing on

DO $$
DECLARE
    test_author_id int := 1;
    affected_posts int;
    affected_comments int;
    start_time timestamptz;
    end_time timestamptz;
    duration_ms numeric;
    patch jsonb;
BEGIN
    -- Count affected rows
    SELECT COUNT(*) INTO affected_posts
    FROM tv_bench_posts
    WHERE author_id = test_author_id;

    SELECT COUNT(*) INTO affected_comments
    FROM tv_bench_comments c
    JOIN bench_comments bc ON c.id = bc.id
    WHERE bc.author_id = test_author_id;

    RAISE NOTICE 'Testing author %: % posts, % comments affected',
        test_author_id, affected_posts, affected_comments;

    -- Start timing
    start_time := clock_timestamp();

    -- Update author
    UPDATE bench_authors
    SET name = 'Smart Updated Author ' || test_author_id,
        email = 'smart' || test_author_id || '@example.com'
    WHERE id = test_author_id;

    -- Build patch for author update
    SELECT jsonb_build_object(
        'id', id,
        'name', name,
        'email', email
    ) INTO patch
    FROM bench_authors
    WHERE id = test_author_id;

    -- Cascade 1: Update posts using SMART PATCH (nested object)
    UPDATE tv_bench_posts
    SET
        data = jsonb_smart_patch_nested(data, patch, ARRAY['author']),
        updated_at = now()
    WHERE author_id = test_author_id;

    -- Cascade 2: Update comments using SMART PATCH (nested object)
    UPDATE tv_bench_comments tc
    SET
        data = jsonb_smart_patch_nested(data, patch, ARRAY['author']),
        updated_at = now()
    FROM bench_comments bc
    WHERE tc.id = bc.id
        AND bc.author_id = test_author_id;

    -- Cascade 3: Update posts with affected comments using SMART PATCH (array)
    -- For each post, build a patch with the updated comment author
    UPDATE tv_bench_posts tp
    SET
        data = (
            SELECT jsonb_smart_patch_array(
                tp.data,
                jsonb_build_object(
                    'id', bc.id,
                    'author', patch
                ),
                ARRAY['comments'],
                'id'
            )
            FROM bench_comments bc
            WHERE bc.post_id = tp.id
                AND bc.author_id = test_author_id
            LIMIT 1  -- Just update first matching comment as example
        ),
        updated_at = now()
    WHERE EXISTS (
        SELECT 1 FROM bench_comments bc
        WHERE bc.post_id = tp.id
            AND bc.author_id = test_author_id
    );

    -- End timing
    end_time := clock_timestamp();
    duration_ms := EXTRACT(EPOCH FROM (end_time - start_time)) * 1000;

    RAISE NOTICE 'SMART PATCH: %.2f ms', duration_ms;
    RAISE NOTICE '  Posts updated: %', affected_posts;
    RAISE NOTICE '  Comments updated: %', affected_comments;

    -- Rollback for repeatability
    RAISE EXCEPTION 'ROLLBACK - Test complete' USING ERRCODE = 'P0001';
END $$;

\timing off
-- Variance test: Small cascade smart patching
-- Tests smart patching performance with minimal cascade impact

\timing on

DO $$
DECLARE
    test_author_id int;
    affected_posts int;
    affected_comments int;
    start_time timestamptz;
    end_time timestamptz;
    duration_ms numeric;
    patch jsonb;
BEGIN
    -- Find an author with minimal activity
    SELECT a.id INTO test_author_id
    FROM bench_authors a
    LEFT JOIN bench_posts p ON a.id = p.author_id
    LEFT JOIN bench_comments c ON a.id = c.author_id
    GROUP BY a.id
    HAVING COUNT(DISTINCT p.id) <= 2 AND COUNT(DISTINCT c.id) <= 5
    ORDER BY COUNT(DISTINCT p.id) + COUNT(DISTINCT c.id)
    LIMIT 1;

    -- Count affected rows
    SELECT COUNT(*) INTO affected_posts
    FROM tv_bench_posts
    WHERE author_id = test_author_id;

    SELECT COUNT(*) INTO affected_comments
    FROM tv_bench_comments c
    JOIN bench_comments bc ON c.id = bc.id
    WHERE bc.author_id = test_author_id;

    RAISE NOTICE 'SMALL CASCADE - Testing author %: % posts, % comments affected',
        test_author_id, affected_posts, affected_comments;

    -- Start timing
    start_time := clock_timestamp();

    -- Update author
    UPDATE bench_authors
    SET name = 'Smart Small Cascade Author ' || test_author_id,
        email = 'smartsmall' || test_author_id || '@example.com'
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
            LIMIT 1
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

    RAISE NOTICE 'SMALL CASCADE SMART PATCH: %.2f ms (%.2f ms per row)', duration_ms, duration_ms / GREATEST(affected_posts + affected_comments, 1);

    -- Rollback for repeatability
    RAISE EXCEPTION 'ROLLBACK - Test complete' USING ERRCODE = 'P0001';
END $$;

\timing off
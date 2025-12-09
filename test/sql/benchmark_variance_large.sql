-- Variance test: Large cascade (popular author with many posts/comments)
-- Tests performance with ~50 posts + ~250 comments affected

\timing on

DO $$
DECLARE
    test_author_id int := 1;  -- Popular author
    affected_posts int;
    affected_comments int;
    start_time timestamptz;
    end_time timestamptz;
    duration_ms numeric;
BEGIN
    -- Count affected rows for popular author
    SELECT COUNT(*) INTO affected_posts
    FROM tv_bench_posts
    WHERE author_id = test_author_id;

    SELECT COUNT(*) INTO affected_comments
    FROM tv_bench_comments c
    JOIN bench_comments bc ON c.id = bc.id
    WHERE bc.author_id = test_author_id;

    RAISE NOTICE 'LARGE CASCADE - Testing author %: % posts, % comments affected',
        test_author_id, affected_posts, affected_comments;

    -- Start timing
    start_time := clock_timestamp();

    -- Update author
    UPDATE bench_authors
    SET name = 'Large Cascade Author ' || test_author_id,
        email = 'large' || test_author_id || '@example.com'
    WHERE id = test_author_id;

    -- Cascade 1: Update posts (FULL REPLACEMENT)
    UPDATE tv_bench_posts tp
    SET
        data = vp.data,
        updated_at = now()
    FROM v_bench_posts vp
    WHERE tp.id = vp.id
        AND tp.author_id = test_author_id;

    -- Cascade 2: Update comments (FULL REPLACEMENT)
    UPDATE tv_bench_comments tc
    SET
        data = vc.data,
        updated_at = now()
    FROM v_bench_comments vc
    JOIN bench_comments bc ON vc.id = bc.id
    WHERE tc.id = vc.id
        AND bc.author_id = test_author_id;

    -- Cascade 3: Update posts that have updated comments (FULL REPLACEMENT)
    UPDATE tv_bench_posts tp
    SET
        data = vp.data,
        updated_at = now()
    FROM v_bench_posts vp
    WHERE tp.id = vp.id
        AND EXISTS (
            SELECT 1 FROM bench_comments bc
            WHERE bc.post_id = tp.id
                AND bc.author_id = test_author_id
        );

    -- End timing
    end_time := clock_timestamp();
    duration_ms := EXTRACT(EPOCH FROM (end_time - start_time)) * 1000;

    RAISE NOTICE 'LARGE CASCADE BASELINE: %.2f ms (%.2f ms per row)', duration_ms, duration_ms / (affected_posts + affected_comments);

    -- Rollback for repeatability
    RAISE EXCEPTION 'ROLLBACK - Test complete' USING ERRCODE = 'P0001';
END $$;

\timing off
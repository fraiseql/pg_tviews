-- Generate benchmark test data
-- Run this AFTER benchmark_schema.sql

-- 1. Insert authors
INSERT INTO bench_authors (name, email)
SELECT
    'Author ' || i,
    'author' || i || '@example.com'
FROM generate_series(1, 100) AS i;

-- 2. Insert posts (10 posts per author on average)
INSERT INTO bench_posts (author_id, title, content, status)
SELECT
    (random() * 99 + 1)::int AS author_id,
    'Post Title ' || i,
    'Lorem ipsum dolor sit amet, consectetur adipiscing elit. ' ||
    'This is post number ' || i || '. ' ||
    repeat('Content goes here. ', 20),  -- ~400 chars per post
    CASE WHEN random() < 0.8 THEN 'published' ELSE 'draft' END
FROM generate_series(1, 1000) AS i;

-- 3. Insert comments (5 comments per post on average)
INSERT INTO bench_comments (post_id, author_id, content)
SELECT
    (random() * 999 + 1)::int AS post_id,
    (random() * 99 + 1)::int AS author_id,
    'This is comment ' || i || '. ' ||
    repeat('Comment content here. ', 10)  -- ~200 chars per comment
FROM generate_series(1, 5000) AS i;

-- 4. Initial TVIEW population
SELECT refresh_tview_comments();
SELECT refresh_tview_posts();

-- 5. Verify data counts
DO $$
DECLARE
    author_count int;
    post_count int;
    comment_count int;
    tv_comment_count int;
    tv_post_count int;
BEGIN
    SELECT COUNT(*) INTO author_count FROM bench_authors;
    SELECT COUNT(*) INTO post_count FROM bench_posts;
    SELECT COUNT(*) INTO comment_count FROM bench_comments;
    SELECT COUNT(*) INTO tv_comment_count FROM tv_bench_comments;
    SELECT COUNT(*) INTO tv_post_count FROM tv_bench_posts;

    RAISE NOTICE 'Data loaded:';
    RAISE NOTICE '  Authors: %', author_count;
    RAISE NOTICE '  Posts: %', post_count;
    RAISE NOTICE '  Comments: %', comment_count;
    RAISE NOTICE '  TV Comments: %', tv_comment_count;
    RAISE NOTICE '  TV Posts: %', tv_post_count;

    IF author_count < 100 OR post_count < 1000 OR comment_count < 5000 THEN
        RAISE WARNING 'Data counts lower than expected!';
    END IF;
END $$;
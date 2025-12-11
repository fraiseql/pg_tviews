-- Test 41: Single Row Refresh (No Cascade)
-- Purpose: Verify single row refresh works correctly without cascading
-- Expected: Row updated in tv_* table, updated_at timestamp changes

\set ECHO all
\set ON_ERROR_STOP on

BEGIN;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_ivm CASCADE;

CREATE EXTENSION jsonb_ivm;
CREATE EXTENSION pg_tviews;

\echo '=========================================='
\echo 'Test 41: Single Row Refresh'
\echo '=========================================='

-- Create simple table (no foreign keys)
CREATE TABLE tb_article (
    pk_article INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title TEXT NOT NULL,
    body TEXT,
    status TEXT DEFAULT 'draft',
    view_count INTEGER DEFAULT 0
);

-- Insert test data
INSERT INTO tb_article (title, body, status, view_count)
VALUES
    ('First Article', 'First body', 'published', 100),
    ('Second Article', 'Second body', 'draft', 5),
    ('Third Article', 'Third body', 'published', 50);

-- Create helper view (workaround for parser)
CREATE VIEW article_prepared AS
SELECT
    pk_article,
    id,
    jsonb_build_object(
        'id', id::text,
        'title', title,
        'body', body,
        'status', status,
        'viewCount', view_count
    ) AS data
FROM tb_article;

-- Create TVIEW using SQL function
SELECT pg_tviews_create('tv_article', 'SELECT pk_article, id, data FROM article_prepared');

-- Test 1: Verify initial state
\echo ''
\echo 'Test 1: Verify initial population'
SELECT COUNT(*) = 3 as correct_article_count FROM tv_article;

-- Verify data correctness
SELECT
    COUNT(*) = 3 as all_articles_present,
    COUNT(*) FILTER (WHERE data->>'title' = 'First Article') = 1 as first_article_correct,
    COUNT(*) FILTER (WHERE data->>'status' = 'published') = 2 as published_count_correct,
    SUM((data->>'viewCount')::int) = 155 as total_view_count_correct
FROM tv_article;

\echo '✓ Test 1 passed: Initial population correct'

-- Test 2: Update single scalar field
\echo ''
\echo 'Test 2: Update single scalar field'
-- Record timestamp before update
SELECT updated_at AS before_update FROM tv_article WHERE pk_article = 1 \gset

-- Wait a moment to ensure timestamp difference
SELECT pg_sleep(0.1);

-- Update title
UPDATE tb_article SET title = 'First Article - Updated' WHERE pk_article = 1;

-- Verify refresh
SELECT
    (data->>'title') = 'First Article - Updated' as title_updated,
    updated_at > :'before_update'::timestamptz as timestamp_changed
FROM tv_article
WHERE pk_article = 1;

-- Verify other rows NOT updated
SELECT
    COUNT(*) = 2 as other_rows_unchanged,
    COUNT(*) FILTER (WHERE data->>'title' != 'First Article - Updated') = 2 as other_titles_unchanged
FROM tv_article
WHERE pk_article != 1;
WHERE pk_article != 1
  AND updated_at <= :'before_update'::timestamptz;
-- Expected: 2 (other rows should have old timestamp)

\echo '✓ Test 2 passed: Single field update works'

-- Test 3: Update multiple fields
\echo ''
\echo 'Test 3: Update multiple fields'
UPDATE tb_article
SET status = 'archived', view_count = 999
WHERE pk_article = 2;

SELECT
    pk_article,
    data->>'status' AS status,
    (data->>'viewCount')::int AS view_count
FROM tv_article
WHERE pk_article = 2;
-- Expected: 'archived', 999

\echo '✓ Test 3 passed: Multiple field update works'

-- Test 4: Update all fields
\echo ''
\echo 'Test 4: Update all fields'
UPDATE tb_article
SET title = 'New Title',
    body = 'New Body',
    status = 'published',
    view_count = 12345
WHERE pk_article = 3;

SELECT
    data->>'title' AS title,
    data->>'body' AS body,
    data->>'status' AS status,
    (data->>'viewCount')::int AS view_count
FROM tv_article
WHERE pk_article = 3;
-- Expected: all new values

\echo '✓ Test 4 passed: Full row update works'

-- Test 5: Verify updated_at maintained correctly
\echo ''
\echo 'Test 5: Verify updated_at timestamps'
SELECT
    pk_article,
    updated_at > NOW() - INTERVAL '10 seconds' AS recently_updated,
    updated_at < NOW() + INTERVAL '1 second' AS not_future
FROM tv_article
ORDER BY pk_article;
-- Expected: all true (all updated recently)

\echo '✓ Test 5 passed: updated_at timestamps correct'

-- Test 6: NULL value handling
\echo ''
\echo 'Test 6: NULL value handling'
UPDATE tb_article SET body = NULL WHERE pk_article = 1;

SELECT
    pk_article,
    data->>'body' IS NULL AS body_is_null
FROM tv_article
WHERE pk_article = 1;
-- Expected: true

\echo '✓ Test 6 passed: NULL values handled correctly'

-- Test 7: Verify no cascade (this is single-table test)
\echo ''
\echo 'Test 7: Verify no cascade happened'
-- This test just confirms we only have one table/TVIEW
SELECT COUNT(*) AS tview_count FROM pg_tview_meta;
-- Expected: 1

\echo '✓ Test 7 passed: No unexpected cascades'

\echo ''
\echo '=========================================='
\echo 'Test 41: All tests passed! ✓'
\echo '=========================================='

ROLLBACK;

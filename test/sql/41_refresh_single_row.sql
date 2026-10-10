-- Test 41: Single Row Refresh (No Cascade)
-- Purpose: Verify single row refresh works correctly without cascading
-- Expected: Row updated in tv_* table, updated_at timestamp changes

\set ECHO all
\set ON_ERROR_STOP on

BEGIN;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;

CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

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
        'view_count', view_count
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
    SUM((data->>'view_count')::int) = 155 as total_view_count_correct
FROM tv_article;

SELECT assert_fresh('tv_article', 'pk_article', 'pg_tviews_create');
DO $$ BEGIN
    IF (SELECT COUNT(*) FROM tv_article) <> 3
       OR (SELECT SUM((data->>'view_count')::int) FROM tv_article) <> 155 THEN
        RAISE EXCEPTION 'FAIL: tv_article initial population wrong';
    END IF;
END $$;

\echo '✓ Test 1 passed: Initial population correct'

-- Test 2: Update single scalar field
\echo ''
\echo 'Test 2: Update single scalar field'
-- The whole file runs in one transaction, where now() (and so updated_at)
-- never advances: whether a row was rewritten shows in its ctid instead, since
-- every UPDATE writes a new tuple version and a no-op refresh writes none.
CREATE TEMP TABLE article_before AS SELECT pk_article, ctid AS tid FROM tv_article;

-- Update title
UPDATE tb_article SET title = 'First Article - Updated' WHERE pk_article = 1;

-- Verify refresh
SELECT
    (data->>'title') = 'First Article - Updated' as title_updated,
    ctid <> (SELECT tid FROM article_before b WHERE b.pk_article = 1) as row_rewritten
FROM tv_article
WHERE pk_article = 1;

-- Verify other rows NOT updated
SELECT
    COUNT(*) = 2 as other_rows_unchanged,
    COUNT(*) FILTER (WHERE data->>'title' != 'First Article - Updated') = 2 as other_titles_unchanged
FROM tv_article t
JOIN article_before b ON b.pk_article = t.pk_article AND b.tid = t.ctid
WHERE t.pk_article != 1;

SELECT assert_fresh('tv_article', 'pk_article', 'an UPDATE of one title');
DO $$ BEGIN
    IF (SELECT data->>'title' FROM tv_article WHERE pk_article = 1) IS DISTINCT FROM 'First Article - Updated' THEN
        RAISE EXCEPTION 'FAIL: article 1 title not refreshed';
    END IF;
    IF (SELECT t.ctid FROM tv_article t WHERE pk_article = 1)
       = (SELECT tid FROM article_before WHERE pk_article = 1) THEN
        RAISE EXCEPTION 'FAIL: article 1 row not rewritten';
    END IF;
    IF (SELECT COUNT(*) FROM tv_article t
        JOIN article_before b ON b.pk_article = t.pk_article AND b.tid = t.ctid
        WHERE t.pk_article <> 1) <> 2 THEN
        RAISE EXCEPTION 'FAIL: refreshing article 1 rewrote other rows';
    END IF;
END $$;

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
    (data->>'view_count')::int AS view_count
FROM tv_article
WHERE pk_article = 2;

SELECT assert_fresh('tv_article', 'pk_article', 'an UPDATE of two fields');
DO $$ BEGIN
    IF (SELECT (data->>'status', (data->>'view_count')::int) FROM tv_article WHERE pk_article = 2)
       IS DISTINCT FROM ('archived'::text, 999) THEN
        RAISE EXCEPTION 'FAIL: article 2 not (archived, 999)';
    END IF;
END $$;

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
    (data->>'view_count')::int AS view_count
FROM tv_article
WHERE pk_article = 3;

SELECT assert_fresh('tv_article', 'pk_article', 'an UPDATE of every field');
DO $$ BEGIN
    IF (SELECT (data->>'title', data->>'body', data->>'status', (data->>'view_count')::int)
        FROM tv_article WHERE pk_article = 3)
       IS DISTINCT FROM ('New Title'::text, 'New Body'::text, 'published'::text, 12345) THEN
        RAISE EXCEPTION 'FAIL: article 3 does not carry all new values';
    END IF;
END $$;

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

DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tv_article
               WHERE updated_at IS NULL
                  OR updated_at <= NOW() - INTERVAL '10 seconds'
                  OR updated_at >= NOW() + INTERVAL '1 second') THEN
        RAISE EXCEPTION 'FAIL: an updated_at is missing or out of range';
    END IF;
END $$;

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

SELECT assert_fresh('tv_article', 'pk_article', 'an UPDATE to NULL');
DO $$ BEGIN
    IF (SELECT data->>'body' FROM tv_article WHERE pk_article = 1) IS NOT NULL THEN
        RAISE EXCEPTION 'FAIL: article 1 body not NULL';
    END IF;
END $$;

\echo '✓ Test 6 passed: NULL values handled correctly'

-- Test 7: Verify no cascade (this is single-table test)
\echo ''
\echo 'Test 7: Verify no cascade happened'
-- This test just confirms we only have one table/TVIEW
SELECT COUNT(*) AS tview_count FROM pg_tview_meta;

DO $$ BEGIN
    IF (SELECT COUNT(*) FROM pg_tview_meta) <> 1 THEN
        RAISE EXCEPTION 'FAIL: expected exactly one TVIEW';
    END IF;
END $$;

\echo '✓ Test 7 passed: No unexpected cascades'

\echo ''
\echo '=========================================='
\echo 'Test 41: All tests passed! ✓'
\echo '=========================================='

ROLLBACK;

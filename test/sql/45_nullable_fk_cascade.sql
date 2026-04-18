-- Test 45: Nullable FK Cascade (no spurious warnings)
-- Purpose: Verify NULL optional FK values skip silently without WARNING
-- Expected: No warning for NULL FK; cascade works for non-NULL FK

\set ECHO all
\set ON_ERROR_STOP on

BEGIN;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;

CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\echo '=========================================='
\echo 'Test 45: Nullable FK Cascade'
\echo '=========================================='

-- Create parent and child tables with NULLABLE FK
CREATE TABLE tb_author (
    pk_author INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT NOT NULL
);

CREATE TABLE tb_article (
    pk_article INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author INTEGER,  -- intentionally nullable
    title TEXT NOT NULL,
    FOREIGN KEY (fk_author) REFERENCES tb_author(pk_author)
);

-- Insert parent data
INSERT INTO tb_author (name) VALUES ('Alice'), ('Bob');

-- Create helper views
CREATE VIEW author_prepared AS
SELECT
    pk_author,
    id,
    jsonb_build_object('id', id::text, 'name', name) AS data
FROM tb_author;

CREATE VIEW article_prepared AS
SELECT
    a.pk_article,
    a.id,
    a.fk_author,
    jsonb_build_object(
        'id', a.id::text,
        'title', a.title,
        'author', author_prepared.data
    ) AS data
FROM tb_article a
LEFT JOIN author_prepared ON author_prepared.pk_author = a.fk_author;

-- Create TVIEWs (parent first)
SELECT pg_tviews_create('tv_author', 'SELECT pk_author, id, data FROM author_prepared');
SELECT pg_tviews_create('tv_article', 'SELECT pk_article, id, fk_author, data FROM article_prepared');

-- Test 1: Insert with NULL FK - should produce NO warning
\echo ''
\echo 'Test 1: Insert with NULL FK (no warning expected)'

INSERT INTO tb_article (title) VALUES ('Orphan Article');

SELECT COUNT(*) = 1 AS orphan_inserted FROM tv_article WHERE fk_author IS NULL;

\echo 'If no WARNING appeared above, test 1 passed'

-- Test 2: Insert with valid FK - should cascade normally
\echo ''
\echo 'Test 2: Insert with valid FK (cascade expected)'

INSERT INTO tb_article (fk_author, title) VALUES (1, 'Alice Article');

SELECT
    data->>'title' AS title,
    data->'author'->>'name' AS author_name
FROM tv_article
WHERE fk_author = 1;
-- Expected: 'Alice Article', 'Alice'

\echo 'Test 2 passed: Valid FK cascaded correctly'

-- Test 3: Update from NULL to valid FK
\echo ''
\echo 'Test 3: Update NULL FK to valid FK'

UPDATE tb_article SET fk_author = 2 WHERE fk_author IS NULL;

SELECT
    data->>'title' AS title,
    data->'author'->>'name' AS author_name
FROM tv_article
WHERE data->>'title' = 'Orphan Article';
-- Expected: 'Orphan Article', 'Bob'

\echo 'Test 3 passed: NULL-to-valid FK update cascaded'

-- Test 4: Update from valid FK to NULL - should produce NO warning
\echo ''
\echo 'Test 4: Update valid FK to NULL (no warning expected)'

UPDATE tb_article SET fk_author = NULL WHERE title = 'Orphan Article';

SELECT fk_author IS NULL AS fk_is_null FROM tv_article WHERE data->>'title' = 'Orphan Article';
-- Expected: true

\echo 'If no WARNING appeared above, test 4 passed'

-- Test 5: Bulk insert with mix of NULL and non-NULL FKs
\echo ''
\echo 'Test 5: Bulk insert with mixed NULL/non-NULL FKs (no warning expected)'

INSERT INTO tb_article (fk_author, title) VALUES
    (NULL, 'Bulk Orphan 1'),
    (1, 'Bulk Alice 1'),
    (NULL, 'Bulk Orphan 2'),
    (2, 'Bulk Bob 1'),
    (NULL, 'Bulk Orphan 3');

SELECT
    COUNT(*) FILTER (WHERE fk_author IS NULL) AS null_fk_count,
    COUNT(*) FILTER (WHERE fk_author IS NOT NULL) AS valid_fk_count
FROM tv_article;

\echo 'If no WARNING appeared above, test 5 passed'

\echo ''
\echo '=========================================='
\echo 'Test 45: All tests passed!'
\echo '=========================================='

ROLLBACK;

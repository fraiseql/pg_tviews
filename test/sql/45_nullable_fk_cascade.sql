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
\ir lib/assert_fresh.sql

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
SELECT assert_fresh('tv_author', 'pk_author', 'pg_tviews_create');
SELECT assert_fresh('tv_article', 'pk_article', 'pg_tviews_create');

-- Test 1: Insert with NULL FK - should produce NO warning
\echo ''
\echo 'Test 1: Insert with NULL FK (no warning expected)'

INSERT INTO tb_article (title) VALUES ('Orphan Article');

SELECT COUNT(*) = 1 AS orphan_inserted FROM tv_article WHERE fk_author IS NULL;

-- A WARNING cannot be trapped from SQL; what is asserted is the row itself.
SELECT assert_fresh('tv_author', 'pk_author', 'an INSERT with a NULL FK');
SELECT assert_fresh('tv_article', 'pk_article', 'an INSERT with a NULL FK');
DO $$ BEGIN
    IF (SELECT COUNT(*) FROM tv_article WHERE fk_author IS NULL) <> 1 THEN
        RAISE EXCEPTION 'FAIL: orphan article missing from tv_article';
    END IF;
    IF (SELECT data->'author' FROM tv_article WHERE data->>'title' = 'Orphan Article') <> 'null'::jsonb THEN
        RAISE EXCEPTION 'FAIL: orphan article author is not JSON null';
    END IF;
END $$;

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

SELECT assert_fresh('tv_author', 'pk_author', 'an INSERT with a valid FK');
SELECT assert_fresh('tv_article', 'pk_article', 'an INSERT with a valid FK');
DO $$ BEGIN
    IF (SELECT data->'author'->>'name' FROM tv_article WHERE fk_author = 1) IS DISTINCT FROM 'Alice' THEN
        RAISE EXCEPTION 'FAIL: Alice''s article does not embed Alice';
    END IF;
END $$;

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

SELECT assert_fresh('tv_author', 'pk_author', 'an UPDATE of a NULL FK to a valid one');
SELECT assert_fresh('tv_article', 'pk_article', 'an UPDATE of a NULL FK to a valid one');
DO $$ BEGIN
    IF (SELECT data->'author'->>'name' FROM tv_article WHERE data->>'title' = 'Orphan Article')
       IS DISTINCT FROM 'Bob' THEN
        RAISE EXCEPTION 'FAIL: formerly orphan article does not embed Bob';
    END IF;
END $$;

\echo 'Test 3 passed: NULL-to-valid FK update cascaded'

-- Test 4: Update from valid FK to NULL - should produce NO warning
\echo ''
\echo 'Test 4: Update valid FK to NULL (no warning expected)'

UPDATE tb_article SET fk_author = NULL WHERE title = 'Orphan Article';

SELECT fk_author IS NULL AS fk_is_null FROM tv_article WHERE data->>'title' = 'Orphan Article';
-- Expected: true

SELECT assert_fresh('tv_author', 'pk_author', 'an UPDATE of a valid FK to NULL');
SELECT assert_fresh('tv_article', 'pk_article', 'an UPDATE of a valid FK to NULL');
DO $$ BEGIN
    IF (SELECT (fk_author IS NULL AND data->'author' = 'null'::jsonb)
        FROM tv_article WHERE data->>'title' = 'Orphan Article') IS NOT TRUE THEN
        RAISE EXCEPTION 'FAIL: article set back to NULL FK still embeds an author';
    END IF;
END $$;

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

SELECT assert_fresh('tv_author', 'pk_author', 'a mixed NULL/non-NULL bulk INSERT');
SELECT assert_fresh('tv_article', 'pk_article', 'a mixed NULL/non-NULL bulk INSERT');
DO $$ BEGIN
    IF (SELECT (COUNT(*) FILTER (WHERE fk_author IS NULL), COUNT(*) FILTER (WHERE fk_author IS NOT NULL))
        FROM tv_article) IS DISTINCT FROM (4::bigint, 3::bigint) THEN
        RAISE EXCEPTION 'FAIL: expected 4 NULL-FK and 3 valid-FK articles';
    END IF;
    IF (SELECT data->'author'->>'name' FROM tv_article WHERE data->>'title' = 'Bulk Bob 1') IS DISTINCT FROM 'Bob' THEN
        RAISE EXCEPTION 'FAIL: bulk-inserted Bob article does not embed Bob';
    END IF;
END $$;

\echo 'If no WARNING appeared above, test 5 passed'

\echo ''
\echo '=========================================='
\echo 'Test 45: All tests passed!'
\echo '=========================================='

ROLLBACK;

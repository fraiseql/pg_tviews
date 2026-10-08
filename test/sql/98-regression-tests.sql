-- Regression tests to prevent future breakage
\set ON_ERROR_STOP on
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

\echo 'Regression Test Suite: jsonb_delta enhancements'

CREATE EXTENSION IF NOT EXISTS pg_tviews;  -- NO CASCADE for regression testing
\ir lib/assert_fresh.sql

\echo ''
\echo '### Regression 1: Fallback when jsonb_delta not installed'

-- pg_tviews is installed without jsonb_delta here; a TVIEW must still be
-- created and refreshed (by replacing documents instead of patching them).
DO $$ BEGIN
    IF pg_tviews_check_jsonb_delta() THEN
        RAISE EXCEPTION 'FAIL: jsonb_delta is installed; this regression needs it absent';
    END IF;
END $$;

CREATE TABLE test_fallback (
    pk_test BIGINT PRIMARY KEY,
    data JSONB
);

INSERT INTO test_fallback VALUES (1, '{"id": "test_123", "items": []}'::jsonb);

-- These should work even without jsonb_delta (graceful degradation)
SELECT pg_tviews_create('fallback', $$
    SELECT pk_test AS pk_fallback, data FROM test_fallback
$$);
SELECT assert_fresh('tv_fallback', 'pk_fallback', 'pg_tviews_create without jsonb_delta');

UPDATE test_fallback
SET data = jsonb_set(data, '{items}', data->'items' || '[{"id": "item_1"}]'::jsonb)
WHERE pk_test = 1;
SELECT assert_fresh('tv_fallback', 'pk_fallback', 'UPDATE test_fallback');

INSERT INTO test_fallback VALUES (2, '{"id": "test_456", "items": []}'::jsonb);
SELECT assert_fresh('tv_fallback', 'pk_fallback', 'INSERT INTO test_fallback');

DELETE FROM test_fallback WHERE pk_test = 2;
SELECT assert_fresh('tv_fallback', 'pk_fallback', 'DELETE FROM test_fallback');

DO $$
BEGIN
    IF (SELECT data->'items'->0->>'id' FROM tv_fallback WHERE pk_fallback = 1)
       IS DISTINCT FROM 'item_1'
       OR (SELECT count(*) FROM tv_fallback) <> 1 THEN
        RAISE EXCEPTION 'FAIL: tv_fallback not refreshed without jsonb_delta';
    END IF;
    RAISE NOTICE 'PASS: Fallback logic works when jsonb_delta unavailable';
END $$;

SELECT pg_tviews_drop('fallback');
DROP TABLE test_fallback;

\echo ''
\echo '### Regression 2: Existing functionality unchanged'

-- Test that old behavior still works
CREATE TABLE test_existing (
    pk_test BIGINT PRIMARY KEY,
    data JSONB
);

INSERT INTO test_existing VALUES (1, '{"name": "test"}'::jsonb);

UPDATE test_existing
SET data = jsonb_set(data, '{name}', '"updated"'::jsonb)
WHERE pk_test = 1;

DO $$
DECLARE
    name text;
BEGIN
    SELECT data->>'name' INTO name FROM test_existing WHERE pk_test = 1;
    IF name = 'updated' THEN
        RAISE NOTICE 'PASS: Standard jsonb_set still works';
    ELSE
        RAISE EXCEPTION 'FAIL: Standard operations broken';
    END IF;
END $$;

DROP TABLE test_existing;

\echo ''
\echo '### Regression 3: Backward compatibility'

-- Test that existing TVIEWs continue to work: the original
-- CREATE TABLE tv_<entity> AS form still registers a TVIEW whose child-table
-- writes refresh it.
CREATE TABLE tb_author (
    pk_author BIGINT PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    name TEXT
);
CREATE TABLE tb_book (
    pk_book BIGINT PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    fk_author BIGINT REFERENCES tb_author(pk_author),
    title TEXT
);
INSERT INTO tb_author (pk_author, name) VALUES (1, 'Ada');
INSERT INTO tb_book (pk_book, fk_author, title) VALUES (1, 1, 'Notes');

CREATE TABLE tv_author AS
SELECT a.pk_author, a.id,
       jsonb_build_object(
           'name', a.name,
           'books', COALESCE(jsonb_agg(b.title ORDER BY b.pk_book)
                             FILTER (WHERE b.pk_book IS NOT NULL), '[]'::jsonb)
       ) AS data
FROM tb_author a
LEFT JOIN tb_book b ON b.fk_author = a.pk_author
GROUP BY a.pk_author, a.id, a.name;
SELECT assert_fresh('tv_author', 'pk_author', 'CREATE TABLE tv_author AS');

INSERT INTO tb_book (pk_book, fk_author, title) VALUES (2, 1, 'Sketch');
SELECT assert_fresh('tv_author', 'pk_author', 'INSERT INTO tb_book');

UPDATE tb_author SET name = 'Ada L.' WHERE pk_author = 1;
SELECT assert_fresh('tv_author', 'pk_author', 'UPDATE tb_author');

DO $$
BEGIN
    IF (SELECT data FROM tv_author WHERE pk_author = 1)
       IS DISTINCT FROM '{"name": "Ada L.", "books": ["Notes", "Sketch"]}'::jsonb THEN
        RAISE EXCEPTION 'FAIL: tv_author not refreshed by writes to tb_author and tb_book';
    END IF;
    RAISE NOTICE 'PASS: Backward compatibility maintained';
END $$;

DROP TABLE tv_author;
DROP TABLE tb_book;
DROP TABLE tb_author;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_tview_meta) THEN
        RAISE EXCEPTION 'FAIL: a TVIEW is still registered after the cleanup';
    END IF;
END $$;

\echo ''
\echo '✓ All regression tests passed'
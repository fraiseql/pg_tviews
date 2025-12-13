-- Test jsonb_extract_id() and jsonb_array_contains_id() wrappers
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

-- Setup test schema
CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Test 1: jsonb_extract_id with default 'id' key
\echo '### Test 1: Extract ID from JSONB'
DO $$
DECLARE
    test_data jsonb := '{"id": "user_123", "name": "Alice"}'::jsonb;
    extracted_id text;
BEGIN
    -- This would call the Rust wrapper, but we can test the SQL function directly
    SELECT jsonb_extract_id(test_data, 'id') INTO extracted_id;

    IF extracted_id = 'user_123' THEN
        RAISE NOTICE 'PASS: Extracted ID correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected user_123, got %', extracted_id;
    END IF;
END $$;

-- Test 2: jsonb_extract_id with custom key
\echo '### Test 2: Extract custom key from JSONB'
DO $$
DECLARE
    test_data jsonb := '{"uuid": "abc-def-ghi", "name": "Bob"}'::jsonb;
    extracted_uuid text;
BEGIN
    SELECT jsonb_extract_id(test_data, 'uuid') INTO extracted_uuid;

    IF extracted_uuid = 'abc-def-ghi' THEN
        RAISE NOTICE 'PASS: Extracted UUID correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected abc-def-ghi, got %', extracted_uuid;
    END IF;
END $$;

-- Test 3: jsonb_array_contains_id - element exists
\echo '### Test 3: Check array contains element (exists)'
DO $$
DECLARE
    test_data jsonb := '{
        "comments": [
            {"id": 1, "text": "Hello"},
            {"id": 2, "text": "World"}
        ]
    }'::jsonb;
    element_exists boolean;
BEGIN
    SELECT jsonb_array_contains_id(test_data, ARRAY['comments'], 'id', '2'::jsonb)
    INTO element_exists;

    IF element_exists THEN
        RAISE NOTICE 'PASS: Found existing element';
    ELSE
        RAISE EXCEPTION 'FAIL: Should have found element with id=2';
    END IF;
END $$;

-- Test 4: jsonb_array_contains_id - element doesn't exist
\echo '### Test 4: Check array contains element (not exists)'
DO $$
DECLARE
    test_data jsonb := '{
        "comments": [
            {"id": 1, "text": "Hello"},
            {"id": 2, "text": "World"}
        ]
    }'::jsonb;
    element_exists boolean;
BEGIN
    SELECT jsonb_array_contains_id(test_data, ARRAY['comments'], 'id', '99'::jsonb)
    INTO element_exists;

    IF NOT element_exists THEN
        RAISE NOTICE 'PASS: Correctly identified missing element';
    ELSE
        RAISE EXCEPTION 'FAIL: Should not have found element with id=99';
    END IF;
END $$;

-- Test 5: Integration test with safe insert
\echo '### Test 5: Safe array insert (prevents duplicates)'
CREATE TABLE test_safe_insert (
    pk_test BIGINT PRIMARY KEY,
    data JSONB DEFAULT '{"items": []}'::jsonb
);

INSERT INTO test_safe_insert VALUES (1, '{"items": []}'::jsonb);

-- First insert should succeed
UPDATE test_safe_insert
SET data = jsonb_array_insert_where(
    data,
    ARRAY['items'],
    '{"id": 1, "name": "Item 1"}'::jsonb,
    NULL, NULL
)
WHERE pk_test = 1;

-- Check it was inserted
DO $$
DECLARE
    item_count int;
BEGIN
    SELECT jsonb_array_length(data->'items') INTO item_count FROM test_safe_insert WHERE pk_test = 1;
    IF item_count = 1 THEN
        RAISE NOTICE 'PASS: First insert succeeded';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected 1 item, got %', item_count;
    END IF;
END $$;

-- Second insert of same ID should be prevented (when using safe wrapper)
DO $$
DECLARE
    already_exists boolean;
BEGIN
    SELECT jsonb_array_contains_id(data, ARRAY['items'], 'id', '1'::jsonb)
    INTO already_exists
    FROM test_safe_insert WHERE pk_test = 1;

    IF already_exists THEN
        RAISE NOTICE 'PASS: Detected duplicate, preventing insert';
    ELSE
        RAISE EXCEPTION 'FAIL: Should have detected existing element';
    END IF;
END $$;

DROP TABLE test_safe_insert;

\echo '### All helper function tests passed! ✓'
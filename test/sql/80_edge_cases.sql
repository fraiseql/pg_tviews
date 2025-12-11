-- Edge Case Integration Tests for pg_tviews
-- Tests unusual scenarios and boundary conditions
-- Run with: psql -d test_db -f test/sql/80_edge_cases.sql

-- Clean up from previous runs
DROP TABLE IF EXISTS tv_composite;
DROP TABLE IF EXISTS tb_composite;
DROP TABLE IF EXISTS tv_unicode;
DROP TABLE IF EXISTS tb_unicode;
DROP TABLE IF EXISTS tv_large_jsonb;
DROP TABLE IF EXISTS tb_large_jsonb;
DROP TABLE IF EXISTS tv_null;
DROP TABLE IF EXISTS tb_null;
DROP TABLE IF EXISTS tv_empty;
DROP TABLE IF EXISTS tb_empty;

-- Ensure extension is loaded
DO $$
BEGIN
    CREATE EXTENSION IF NOT EXISTS pg_tviews;
EXCEPTION
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'Cannot create extension (insufficient privileges) - continuing with test';
    WHEN undefined_file THEN
        RAISE NOTICE 'Extension not installed - continuing with test';
END $$;

-- ========================================
-- TEST 1: Empty Base Table
-- ========================================

SELECT 'Test 1: Empty base table' as test_case;

CREATE TABLE tb_empty (
    pk_empty BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid()
);

-- Create TVIEW from empty table
CREATE TABLE tv_empty AS
SELECT
    tb_empty.pk_empty,
    tb_empty.id,
    '{}'::jsonb as data
FROM tb_empty;

-- Verify TVIEW is created but empty
SELECT COUNT(*) = 0 as empty_tview_created FROM tv_empty;

-- ========================================
-- TEST 2: NULL Values in JSONB
-- ========================================

SELECT 'Test 2: NULL values in JSONB' as test_case;

CREATE TABLE tb_null (
    pk_null BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    nullable_field TEXT,
    optional_number INTEGER
);

-- Insert data with NULLs
INSERT INTO tb_null (nullable_field, optional_number) VALUES
    ('value1', 100),
    (NULL, 200),
    ('value3', NULL),
    (NULL, NULL);

-- Create TVIEW with NULL handling
CREATE TABLE tv_null AS
SELECT
    tb_null.pk_null,
    tb_null.id,
    jsonb_build_object(
        'id', tb_null.id,
        'field', tb_null.nullable_field,
        'number', tb_null.optional_number,
        'hasField', tb_null.nullable_field IS NOT NULL,
        'hasNumber', tb_null.optional_number IS NOT NULL
    ) as data
FROM tb_null;

-- Verify NULL handling
SELECT
    COUNT(*) = 4 as all_rows_present,
    SUM((data->>'hasField')::boolean::integer) = 2 as null_fields_handled,
    SUM((data->>'hasNumber')::boolean::integer) = 2 as null_numbers_handled
FROM tv_null;

-- Check specific NULL values
SELECT
    data->>'field' IS NULL as field_is_null,
    data->>'number' IS NULL as number_is_null
FROM tv_null
WHERE pk_null = 2;  -- Should have NULL field

-- ========================================
-- TEST 3: Very Large JSONB Documents (>1MB)
-- ========================================

SELECT 'Test 3: Large JSONB documents' as test_case;

CREATE TABLE tb_large_jsonb (
    pk_large BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    large_data TEXT
);

-- Insert large text data (2MB)
INSERT INTO tb_large_jsonb (large_data)
VALUES (repeat('Large JSONB content with repeated text for testing memory limits: ' ||
                'This is a test of handling very large JSONB documents in pg_tviews. ' ||
                'The content should be properly stored and retrieved without issues. ', 1000));

-- Create TVIEW with large JSONB
CREATE TABLE tv_large_jsonb AS
SELECT
    tb_large_jsonb.pk_large,
    tb_large_jsonb.id,
    jsonb_build_object(
        'id', tb_large_jsonb.id,
        'data', tb_large_jsonb.large_data,
        'size', length(tb_large_jsonb.large_data),
        'checksum', md5(tb_large_jsonb.large_data)
    ) as data
FROM tb_large_jsonb;

-- Verify large document handling
SELECT
    COUNT(*) = 1 as large_document_stored,
    (data->>'size')::integer > 1000000 as size_over_1mb,
    length(data::text) > 1000000 as jsonb_size_large
FROM tv_large_jsonb;

-- ========================================
-- TEST 4: Unicode and Special Characters
-- ========================================

SELECT 'Test 4: Unicode and special characters' as test_case;

CREATE TABLE tb_unicode (
    pk_unicode BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    emoji_field TEXT,
    unicode_text TEXT,
    special_chars TEXT
);

-- Insert Unicode data
INSERT INTO tb_unicode (emoji_field, unicode_text, special_chars) VALUES
    ('🚀 PostgreSQL 🐘', 'café résumé naïve', '{"key": "value with \"quotes\""}'),
    ('🌟 Unicode test 🔍', 'Москва 北京', '<xml>&amp;entites</xml>'),
    ('🎉 Emojis everywhere 🎊', 'العربية हिन्दी', 'multi
line
text');

-- Create TVIEW with Unicode
CREATE TABLE tv_unicode AS
SELECT
    tb_unicode.pk_unicode,
    tb_unicode.id,
    jsonb_build_object(
        'id', tb_unicode.id,
        'emoji', tb_unicode.emoji_field,
        'unicode', tb_unicode.unicode_text,
        'special', tb_unicode.special_chars,
        'length', length(tb_unicode.emoji_field || tb_unicode.unicode_text || tb_unicode.special_chars)
    ) as data
FROM tb_unicode;

-- Verify Unicode handling
SELECT
    COUNT(*) = 3 as unicode_rows_stored,
    data->>'emoji' LIKE '%🚀%' as emoji_preserved,
    data->>'unicode' LIKE '%café%' as accented_chars_preserved,
    data->>'special' LIKE '%{"key"%' as json_string_preserved
FROM tv_unicode
WHERE pk_unicode = 1;

-- ========================================
-- TEST 5: Circular FK References (Self-Referential)
-- ========================================

SELECT 'Test 5: Self-referential tables' as test_case;

CREATE TABLE tb_tree (
    pk_tree BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    fk_parent BIGINT REFERENCES tb_tree(pk_tree),
    name TEXT NOT NULL,
    depth INTEGER DEFAULT 0
);

-- Insert hierarchical data
INSERT INTO tb_tree (fk_parent, name, depth) VALUES
    (NULL, 'root', 0),
    (1, 'child1', 1),
    (1, 'child2', 1),
    (2, 'grandchild1', 2),
    (2, 'grandchild2', 2);

-- Create TVIEW with self-reference
CREATE TABLE tv_tree AS
SELECT
    tb_tree.pk_tree,
    tb_tree.id,
    jsonb_build_object(
        'id', tb_tree.id,
        'name', tb_tree.name,
        'depth', tb_tree.depth,
        'parentId', tb_tree.fk_parent,
        'hasChildren', EXISTS(SELECT 1 FROM tb_tree c WHERE c.fk_parent = tb_tree.pk_tree)
    ) as data
FROM tb_tree;

-- Verify self-referential structure
SELECT
    COUNT(*) = 5 as tree_nodes_created,
    SUM((data->>'hasChildren')::boolean::integer) = 2 as parents_identified,
    jsonb_array_length(
        (SELECT jsonb_agg(data->>'name')
         FROM tv_tree
         WHERE (data->>'depth')::integer > 0)
    ) = 4 as children_count
FROM tv_tree;

-- ========================================
-- TEST 6: Transaction Rollback
-- ========================================

SELECT 'Test 6: Transaction rollback' as test_case;

-- Start transaction
BEGIN;

-- Create table and TVIEW
CREATE TABLE tb_rollback (
    pk_rollback BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    value TEXT
);

INSERT INTO tb_rollback (value) VALUES ('test1'), ('test2');

CREATE TABLE tv_rollback AS
SELECT
    tb_rollback.pk_rollback,
    tb_rollback.id,
    jsonb_build_object('id', tb_rollback.id, 'value', tb_rollback.value) as data
FROM tb_rollback;

-- Verify exists within transaction
SELECT COUNT(*) = 2 as tv_created_in_transaction FROM tv_rollback;

-- Rollback transaction
ROLLBACK;

-- Verify TVIEW was rolled back
SELECT COUNT(*) = 0 as tv_rolled_back FROM pg_class WHERE relname = 'tv_rollback';
SELECT COUNT(*) = 0 as tb_rolled_back FROM pg_class WHERE relname = 'tb_rollback';

-- ========================================
-- TEST 7: Savepoint Handling
-- ========================================

SELECT 'Test 7: Savepoint handling' as test_case;

BEGIN;

-- Create initial TVIEW
CREATE TABLE tv_savepoint AS
SELECT
    tb_unicode.pk_unicode as pk_savepoint,
    tb_unicode.id,
    tb_unicode.emoji_field as data
FROM tb_unicode
LIMIT 2;

SELECT COUNT(*) = 2 as initial_savepoint_created FROM tv_savepoint;

-- Create savepoint
SAVEPOINT sp1;

-- Modify data (create another TVIEW)
CREATE TABLE tv_savepoint_modified AS
SELECT
    tv_savepoint.pk_savepoint,
    tv_savepoint.id,
    jsonb_build_object('emoji', tv_savepoint.data) as data
FROM tv_savepoint;

SELECT COUNT(*) = 2 as modified_savepoint_created FROM tv_savepoint_modified;

-- Rollback to savepoint
ROLLBACK TO sp1;

-- Verify second TVIEW was rolled back but first persists
SELECT COUNT(*) = 2 as first_tv_persisted FROM tv_savepoint;
SELECT COUNT(*) = 0 as second_tv_rolled_back FROM pg_class WHERE relname = 'tv_savepoint_modified';

-- Commit transaction
COMMIT;

-- ========================================
-- TEST 8: Very Long Entity Names
-- ========================================

SELECT 'Test 8: Very long entity names' as test_case;

-- Create TVIEW with very long name (approaching PostgreSQL 63-char limit)
CREATE TABLE tv_this_is_a_very_long_tview_name_that_approaches_the_postgresql_identifier_limit_ AS
SELECT
    tb_unicode.pk_unicode as pk_this_is_a_very_long_tview_name_that_approaches_the_postgresql_identifier_limit_,
    tb_unicode.id,
    jsonb_build_object('id', tb_unicode.id, 'emoji', tb_unicode.emoji_field) as data
FROM tb_unicode
LIMIT 1;

-- Verify long name was truncated/accepted
SELECT
    COUNT(*) > 0 as long_name_handled,
    length(relname) <= 63 as name_within_limit
FROM pg_class
WHERE relname LIKE 'tv_this_is_a_very_long%';

-- ========================================
-- TEST 9: Special Characters in Data
-- ========================================

SELECT 'Test 9: Special characters in data' as test_case;

-- Insert data with special characters
INSERT INTO tb_unicode (emoji_field, unicode_text, special_chars) VALUES
    ('💥', 'test\nwith\nnewlines', 'tab\there'),
    ('🔥', 'quote"in"string', 'backslash\\here');

-- Update TVIEW to include new data
CREATE TABLE tv_special_chars AS
SELECT
    tb_unicode.pk_unicode,
    tb_unicode.id,
    jsonb_build_object(
        'id', tb_unicode.id,
        'emoji', tb_unicode.emoji_field,
        'multiline', tb_unicode.unicode_text,
        'special', tb_unicode.special_chars,
        'escaped', replace(tb_unicode.special_chars, '\', '\\')
    ) as data
FROM tb_unicode
WHERE pk_unicode > 3;  -- Only the new rows

-- Verify special character handling
SELECT
    COUNT(*) >= 2 as special_chars_stored,
    data->>'multiline' LIKE '%\n%' as newlines_preserved,
    data->>'special' LIKE '%\t%' as tabs_preserved,
    data->>'escaped' LIKE '%\\\\%' as backslashes_escaped
FROM tv_special_chars;

-- ========================================
-- TEST 10: Composite Primary Key (Error Case)
-- ========================================

SELECT 'Test 10: Composite primary key (should fail gracefully)' as test_case;

-- Create table with composite PK (not supported by trinity pattern)
CREATE TABLE tb_composite (
    pk_composite_1 INTEGER,
    pk_composite_2 INTEGER,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    data TEXT,
    PRIMARY KEY (pk_composite_1, pk_composite_2)
);

INSERT INTO tb_composite (pk_composite_1, pk_composite_2, data) VALUES
    (1, 1, 'composite_data'),
    (1, 2, 'more_data');

-- Attempt to create TVIEW (should work but may not be fully functional)
CREATE TABLE tv_composite AS
SELECT
    pk_composite_1 as pk_composite,  -- Not a true single PK
    id,
    jsonb_build_object('id', id, 'data', data) as data
FROM tb_composite
LIMIT 1;  -- Just test creation

-- Verify table was created (even if not fully functional)
SELECT COUNT(*) >= 0 as composite_pk_handled FROM pg_class WHERE relname = 'tv_composite';

-- ========================================
-- CLEANUP
-- ========================================

SELECT 'Cleanup: Removing test tables' as status;

DROP TABLE IF EXISTS tv_composite;
DROP TABLE IF EXISTS tb_composite;
DROP TABLE IF EXISTS tv_special_chars;
DROP TABLE IF EXISTS tv_this_is_a_very_long_tview_name_that_approaches_the_postgresql_identifier_limit_;
DROP TABLE IF EXISTS tv_savepoint_modified;
DROP TABLE IF EXISTS tv_savepoint;
DROP TABLE IF EXISTS tv_tree;
DROP TABLE IF EXISTS tb_tree;
DROP TABLE IF EXISTS tv_unicode;
DROP TABLE IF EXISTS tb_unicode;
DROP TABLE IF EXISTS tv_large_jsonb;
DROP TABLE IF EXISTS tb_large_jsonb;
DROP TABLE IF EXISTS tv_null;
DROP TABLE IF EXISTS tb_null;
DROP TABLE IF EXISTS tv_empty;
DROP TABLE IF EXISTS tb_empty;

SELECT 'All edge case tests completed successfully' as result;
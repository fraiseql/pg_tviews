-- Documents of unusual content are materialized and refreshed as the view says:
--
-- 1. an empty base table gives an empty TVIEW, and its first insert appears;
-- 2. SQL NULLs become JSON nulls (NULL ->> 'field');
-- 3. a document over 1 MB;
-- 4. emoji, non-Latin scripts, quotes, markup, newlines and tabs, kept byte for byte;
-- 5. a self-referencing table read again in an EXISTS subquery;
-- 6. a JSONB column passed through whole: a jsonb_set() of a nested key on 50 of
--    1000 rows refreshes exactly those 50.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_document_values.sql
-- expect-output: document_values: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'document_values FAIL: %', what; END IF; END $$;

-- 1. Empty.
CREATE TABLE tb_empty (pk_empty bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE);
CREATE TABLE tv_empty AS SELECT pk_empty, id, '{}'::jsonb AS data FROM tb_empty;
SELECT must((SELECT count(*) FROM tv_empty) = 0, 'tv_empty not empty');
INSERT INTO tb_empty (pk_empty) VALUES (1);
SELECT assert_fresh('tv_empty', 'pk_empty', 'the first insert into an empty table');

-- 2. NULLs.
CREATE TABLE tb_null (pk_null bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                      nullable_field text, optional_number int);
INSERT INTO tb_null (pk_null, nullable_field, optional_number) VALUES
    (1, 'value1', 100), (2, NULL, 200), (3, 'value3', NULL), (4, NULL, NULL);
CREATE TABLE tv_null AS
SELECT pk_null, id, jsonb_build_object('field', nullable_field, 'number', optional_number,
                                       'has_field', nullable_field IS NOT NULL) AS data
FROM tb_null;
SELECT must((SELECT count(*) FROM tv_null) = 4, 'tv_null row count');
SELECT must((SELECT sum((data->>'has_field')::boolean::int) FROM tv_null) = 2, 'NULL field handling');
SELECT must((SELECT data->'field' = 'null'::jsonb FROM tv_null WHERE pk_null = 2), 'NULL is not a JSON null');
UPDATE tb_null SET nullable_field = NULL WHERE pk_null = 1;
SELECT must((SELECT data->>'field' IS NULL FROM tv_null WHERE pk_null = 1), 'update to NULL not refreshed');
SELECT assert_fresh('tv_null', 'pk_null', 'an update to NULL');

-- 3. Over 1 MB.
CREATE TABLE tb_large (pk_large bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE, large_data text);
INSERT INTO tb_large (pk_large, large_data)
VALUES (1, repeat('Large JSONB content for testing memory limits in pg_tviews. ', 30000));
CREATE TABLE tv_large AS
SELECT pk_large, id, jsonb_build_object('size', length(large_data), 'checksum', md5(large_data),
                                        'data', large_data) AS data
FROM tb_large;
SELECT must((SELECT (data->>'size')::int FROM tv_large) > 1000000, 'expected a document over 1 MB');
UPDATE tb_large SET large_data = large_data || 'x';
SELECT assert_fresh('tv_large', 'pk_large', 'an update of a 1 MB document');

-- 4. Unicode and special characters.
CREATE TABLE tb_unicode (pk_unicode bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                         emoji_field text, unicode_text text, special_chars text);
INSERT INTO tb_unicode (pk_unicode, emoji_field, unicode_text, special_chars) VALUES
    (1, '🚀 PostgreSQL 🐘', 'café résumé naïve', '{"key": "value with \"quotes\""}'),
    (2, '🌟 Unicode test 🔍', 'Москва 北京', '<xml>&amp;entities</xml>'),
    (3, '🎉 Emojis 🎊', 'العربية हिन्दी', E'multi\nline\ttext');
CREATE TABLE tv_unicode AS
SELECT pk_unicode, id, jsonb_build_object('emoji', emoji_field, 'unicode', unicode_text,
                                          'special', special_chars) AS data
FROM tb_unicode;
SELECT must((SELECT count(*) FROM tv_unicode) = 3, 'tv_unicode row count');
SELECT must((SELECT data->>'emoji' FROM tv_unicode WHERE pk_unicode = 1) LIKE '%🚀%', 'emoji not preserved');
SELECT must((SELECT data->>'unicode' FROM tv_unicode WHERE pk_unicode = 1) LIKE '%café%', 'accents not preserved');
UPDATE tb_unicode SET unicode_text = unicode_text || ' 東京' WHERE pk_unicode = 2;
SELECT must((SELECT data->>'unicode' FROM tv_unicode WHERE pk_unicode = 2) = 'Москва 北京 東京',
            'unicode update not refreshed');
SELECT assert_fresh('tv_unicode', 'pk_unicode', 'a unicode update');

-- 5. Self-referencing table.
CREATE TABLE tb_tree (pk_tree bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                      fk_parent bigint REFERENCES tb_tree(pk_tree), name text NOT NULL, depth int DEFAULT 0);
INSERT INTO tb_tree (pk_tree, fk_parent, name, depth) VALUES
    (1, NULL, 'root', 0), (2, 1, 'child1', 1), (3, 1, 'child2', 1),
    (4, 2, 'grandchild1', 2), (5, 2, 'grandchild2', 2);
CREATE TABLE tv_tree AS
SELECT pk_tree, id, jsonb_build_object(
        'name', name, 'depth', depth, 'parent_id', fk_parent,
        'has_children', EXISTS (SELECT 1 FROM tb_tree c WHERE c.fk_parent = tb_tree.pk_tree)) AS data
FROM tb_tree;
SELECT must((SELECT count(*) FROM tv_tree) = 5, 'tree node count');
SELECT must((SELECT sum((data->>'has_children')::boolean::int) FROM tv_tree) = 2, 'parent identification');

-- 6. A JSONB column passed through whole.
CREATE TABLE tb_perf (pk_perf bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), data jsonb);
INSERT INTO tb_perf (pk_perf, data)
SELECT i, jsonb_build_object('field1', 'value_' || i,
                             'field2', jsonb_build_object('nested1', 'nested_value_' || i, 'nested2', i,
                                                          'array_field', jsonb_build_array('item1', 'item2', i)))
FROM generate_series(1, 1000) i;
SELECT pg_tviews_create('tv_perf', 'SELECT pk_perf, id, data FROM tb_perf');
SELECT must((SELECT count(*) FROM tv_perf) = 1000, 'tv_perf does not hold the 1000 seeded rows');
DO $$ BEGIN
    FOR i IN 1..50 LOOP
        UPDATE tb_perf SET data = jsonb_set(data, '{field2,nested1}', to_jsonb('updated_' || i)) WHERE pk_perf = i;
    END LOOP;
END $$;
SELECT assert_fresh('tv_perf', 'pk_perf', '50 nested jsonb_set updates');
SELECT must((SELECT count(*) FROM tv_perf WHERE data #>> '{field2,nested1}' = 'updated_' || pk_perf) = 50
            AND (SELECT data #>> '{field2,nested1}' FROM tv_perf WHERE pk_perf = 51) = 'nested_value_51',
            'tv_perf does not show exactly rows 1-50 updated');

\echo 'document_values: PASS'

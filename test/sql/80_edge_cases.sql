-- Edge-case integration tests for pg_tviews: empty tables, NULLs, large JSONB,
-- Unicode/special characters, self-referential tables, transaction rollback,
-- savepoints, and composite-PK rejection.
--
-- Standalone (issue #55): self-contained under psql -v ON_ERROR_STOP=1. Rewritten
-- to the supported tb_<entity>/pk_<entity>/id/data convention. Removed sub-tests
-- that relied on unsupported shapes (tview-on-tview, tviews with no tb_<entity>
-- base, text-typed data columns).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/80_edge_cases.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_composite CASCADE;   DROP TABLE IF EXISTS tb_composite CASCADE;
DROP TABLE IF EXISTS tv_tree CASCADE;        DROP TABLE IF EXISTS tb_tree CASCADE;
DROP TABLE IF EXISTS tv_unicode CASCADE;     DROP TABLE IF EXISTS tb_unicode CASCADE;
DROP TABLE IF EXISTS tv_large_jsonb CASCADE; DROP TABLE IF EXISTS tb_large_jsonb CASCADE;
DROP TABLE IF EXISTS tv_null CASCADE;        DROP TABLE IF EXISTS tb_null CASCADE;
DROP TABLE IF EXISTS tv_empty CASCADE;       DROP TABLE IF EXISTS tb_empty CASCADE;

-- TEST 1: Empty base table -> empty tview.
CREATE TABLE tb_empty (pk_empty BIGSERIAL PRIMARY KEY, id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE);
CREATE TABLE tv_empty AS
SELECT pk_empty, id, '{}'::jsonb AS data FROM tb_empty;
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_empty) <> 0 THEN RAISE EXCEPTION 'Test 1 FAIL: tv_empty not empty'; END IF;
END $$;

-- TEST 2: NULL values in JSONB.
CREATE TABLE tb_null (
    pk_null BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    nullable_field TEXT,
    optional_number INTEGER
);
INSERT INTO tb_null (nullable_field, optional_number) VALUES
    ('value1', 100), (NULL, 200), ('value3', NULL), (NULL, NULL);
CREATE TABLE tv_null AS
SELECT pk_null, id,
    jsonb_build_object(
        'field', nullable_field,
        'number', optional_number,
        'has_field', nullable_field IS NOT NULL,
        'has_number', optional_number IS NOT NULL
    ) AS data
FROM tb_null;
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_null) <> 4 THEN RAISE EXCEPTION 'Test 2 FAIL: row count'; END IF;
  IF (SELECT sum((data->>'has_field')::boolean::int) FROM tv_null) <> 2 THEN
    RAISE EXCEPTION 'Test 2 FAIL: null field handling'; END IF;
  IF (SELECT data->>'field' FROM tv_null WHERE pk_null = 2) IS NOT NULL THEN
    RAISE EXCEPTION 'Test 2 FAIL: expected NULL field for pk_null=2'; END IF;
END $$;

-- TEST 3: Large JSONB document (>1MB).
CREATE TABLE tb_large_jsonb (
    pk_large_jsonb BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    large_data TEXT
);
INSERT INTO tb_large_jsonb (large_data)
VALUES (repeat('Large JSONB content for testing memory limits in pg_tviews. ', 30000));
CREATE TABLE tv_large_jsonb AS
SELECT pk_large_jsonb, id,
    jsonb_build_object(
        'size', length(large_data),
        'checksum', md5(large_data),
        'data', large_data
    ) AS data
FROM tb_large_jsonb;
DO $$ BEGIN
  IF (SELECT (data->>'size')::int FROM tv_large_jsonb) <= 1000000 THEN
    RAISE EXCEPTION 'Test 3 FAIL: expected >1MB document'; END IF;
END $$;

-- TEST 4: Unicode and special characters in data.
CREATE TABLE tb_unicode (
    pk_unicode BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    emoji_field TEXT,
    unicode_text TEXT,
    special_chars TEXT
);
INSERT INTO tb_unicode (emoji_field, unicode_text, special_chars) VALUES
    ('🚀 PostgreSQL 🐘', 'café résumé naïve', '{"key": "value with \"quotes\""}'),
    ('🌟 Unicode test 🔍', 'Москва 北京', '<xml>&amp;entities</xml>'),
    ('🎉 Emojis 🎊', 'العربية हिन्दी', E'multi\nline\ttext');
CREATE TABLE tv_unicode AS
SELECT pk_unicode, id,
    jsonb_build_object(
        'emoji', emoji_field,
        'unicode', unicode_text,
        'special', special_chars
    ) AS data
FROM tb_unicode;
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_unicode) <> 3 THEN RAISE EXCEPTION 'Test 4 FAIL: row count'; END IF;
  IF (SELECT data->>'emoji' FROM tv_unicode WHERE pk_unicode = 1) NOT LIKE '%🚀%' THEN
    RAISE EXCEPTION 'Test 4 FAIL: emoji not preserved'; END IF;
  IF (SELECT data->>'unicode' FROM tv_unicode WHERE pk_unicode = 1) NOT LIKE '%café%' THEN
    RAISE EXCEPTION 'Test 4 FAIL: accented chars not preserved'; END IF;
END $$;

-- TEST 5: Self-referential (tree) table.
CREATE TABLE tb_tree (
    pk_tree BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_parent BIGINT REFERENCES tb_tree(pk_tree),
    name TEXT NOT NULL,
    depth INTEGER DEFAULT 0
);
INSERT INTO tb_tree (fk_parent, name, depth) VALUES
    (NULL, 'root', 0), (1, 'child1', 1), (1, 'child2', 1),
    (2, 'grandchild1', 2), (2, 'grandchild2', 2);
CREATE TABLE tv_tree AS
SELECT pk_tree, id,
    jsonb_build_object(
        'name', name, 'depth', depth, 'parent_id', fk_parent,
        'has_children', EXISTS(SELECT 1 FROM tb_tree c WHERE c.fk_parent = tb_tree.pk_tree)
    ) AS data
FROM tb_tree;
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_tree) <> 5 THEN RAISE EXCEPTION 'Test 5 FAIL: node count'; END IF;
  IF (SELECT sum((data->>'has_children')::boolean::int) FROM tv_tree) <> 2 THEN
    RAISE EXCEPTION 'Test 5 FAIL: parent identification'; END IF;
END $$;

-- TEST 6: Transaction rollback removes the tview and its metadata.
BEGIN;
    CREATE TABLE tb_rollback (pk_rollback BIGSERIAL PRIMARY KEY, id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE, value TEXT);
    INSERT INTO tb_rollback (value) VALUES ('test1'), ('test2');
    CREATE TABLE tv_rollback AS
    SELECT pk_rollback, id, jsonb_build_object('value', value) AS data FROM tb_rollback;
    DO $$ BEGIN
      IF (SELECT count(*) FROM tv_rollback) <> 2 THEN RAISE EXCEPTION 'Test 6 FAIL: in-txn count'; END IF;
    END $$;
ROLLBACK;
DO $$ BEGIN
  IF to_regclass('tv_rollback') IS NOT NULL THEN RAISE EXCEPTION 'Test 6 FAIL: tv_rollback survived ROLLBACK'; END IF;
  IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'rollback') THEN
    RAISE EXCEPTION 'Test 6 FAIL: rollback metadata survived ROLLBACK'; END IF;
END $$;

-- TEST 7: Savepoint (ROLLBACK TO) — the tview created after the savepoint is
-- undone; the one created before it survives, within the same transaction.
BEGIN;
    CREATE TABLE tb_sp_a (pk_sp_a BIGSERIAL PRIMARY KEY, id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE, v TEXT);
    INSERT INTO tb_sp_a (v) VALUES ('a1'), ('a2');
    CREATE TABLE tv_sp_a AS SELECT pk_sp_a, id, jsonb_build_object('v', v) AS data FROM tb_sp_a;

    SAVEPOINT sp1;
    CREATE TABLE tb_sp_b (pk_sp_b BIGSERIAL PRIMARY KEY, id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE, v TEXT);
    INSERT INTO tb_sp_b (v) VALUES ('b1');
    CREATE TABLE tv_sp_b AS SELECT pk_sp_b, id, jsonb_build_object('v', v) AS data FROM tb_sp_b;
    DO $$ BEGIN
      IF (SELECT count(*) FROM tv_sp_b) <> 1 THEN RAISE EXCEPTION 'Test 7 FAIL: tv_sp_b not created'; END IF;
    END $$;
    ROLLBACK TO sp1;

    DO $$ BEGIN
      IF (SELECT count(*) FROM tv_sp_a) <> 2 THEN RAISE EXCEPTION 'Test 7 FAIL: tv_sp_a should persist through savepoint rollback'; END IF;
      IF to_regclass('tv_sp_b') IS NOT NULL THEN RAISE EXCEPTION 'Test 7 FAIL: tv_sp_b should be undone by ROLLBACK TO'; END IF;
    END $$;
COMMIT;

-- TEST 8: Composite primary key is rejected (the trinity pattern needs a single PK).
CREATE TABLE tb_composite (
    pk_composite_1 INTEGER,
    pk_composite_2 INTEGER,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    payload TEXT,
    PRIMARY KEY (pk_composite_1, pk_composite_2)
);
INSERT INTO tb_composite (pk_composite_1, pk_composite_2, payload) VALUES (1, 1, 'x'), (1, 2, 'y');
DO $$
BEGIN
    PERFORM pg_tviews_create('tv_composite', $q$
        SELECT pk_composite_1 AS pk_composite, id,
               jsonb_build_object('payload', payload) AS data
        FROM tb_composite
    $q$);
    RAISE EXCEPTION 'Test 8 FAIL: composite-PK tview was accepted (expected rejection)';
EXCEPTION
    WHEN OTHERS THEN
        IF SQLERRM LIKE '%Test 8 FAIL%' THEN RAISE; END IF;
        RAISE NOTICE 'Test 8 ok: composite-PK tview rejected: %', SQLERRM;
END $$;

-- Cleanup
DROP TABLE IF EXISTS tv_composite CASCADE;   DROP TABLE IF EXISTS tb_composite CASCADE;
DROP TABLE IF EXISTS tb_sp_a CASCADE;
DROP TABLE IF EXISTS tv_tree CASCADE;        DROP TABLE IF EXISTS tb_tree CASCADE;
DROP TABLE IF EXISTS tv_unicode CASCADE;     DROP TABLE IF EXISTS tb_unicode CASCADE;
DROP TABLE IF EXISTS tv_large_jsonb CASCADE; DROP TABLE IF EXISTS tb_large_jsonb CASCADE;
DROP TABLE IF EXISTS tv_null CASCADE;        DROP TABLE IF EXISTS tb_null CASCADE;
DROP TABLE IF EXISTS tv_empty CASCADE;       DROP TABLE IF EXISTS tb_empty CASCADE;

SELECT '80 PASS: edge cases (empty/NULL/large/unicode/tree/rollback/savepoint/composite) handled' AS result;

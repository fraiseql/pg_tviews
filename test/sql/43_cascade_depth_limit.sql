-- Test 43: Cascade Depth Limiting
-- Purpose: Verify cascade depth is limited to prevent infinite loops
-- Expected: a cascade deeper than pg_tviews.max_propagation_depth is refused

\set ECHO all
\set ON_ERROR_STOP on

BEGIN;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;

CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

-- Every level of the chain must match its backing view.
CREATE FUNCTION assert_levels_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    FOR i IN 0..5 LOOP
        PERFORM assert_fresh(format('tv_level_%s', i)::regclass, format('pk_level_%s', i), label);
    END LOOP;
END $$;

\echo '=========================================='
\echo 'Test 43: Cascade Depth Limiting'
\echo '=========================================='

-- Create a dependency chain of 6 levels
-- level_0 -> level_1 -> level_2 -> ... -> level_5

CREATE TABLE tb_level_0 (
    pk_level_0 INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    value TEXT NOT NULL
);

CREATE TABLE tb_level_1 (
    pk_level_1 INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_level_0 INTEGER NOT NULL,
    value TEXT NOT NULL,
    FOREIGN KEY (fk_level_0) REFERENCES tb_level_0(pk_level_0)
);

CREATE TABLE tb_level_2 (
    pk_level_2 INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_level_1 INTEGER NOT NULL,
    value TEXT NOT NULL,
    FOREIGN KEY (fk_level_1) REFERENCES tb_level_1(pk_level_1)
);

CREATE TABLE tb_level_3 (
    pk_level_3 INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_level_2 INTEGER NOT NULL,
    value TEXT NOT NULL,
    FOREIGN KEY (fk_level_2) REFERENCES tb_level_2(pk_level_2)
);

CREATE TABLE tb_level_4 (
    pk_level_4 INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_level_3 INTEGER NOT NULL,
    value TEXT NOT NULL,
    FOREIGN KEY (fk_level_3) REFERENCES tb_level_3(pk_level_3)
);

CREATE TABLE tb_level_5 (
    pk_level_5 INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_level_4 INTEGER NOT NULL,
    value TEXT NOT NULL,
    FOREIGN KEY (fk_level_4) REFERENCES tb_level_4(pk_level_4)
);

-- Insert initial data
INSERT INTO tb_level_0 (value) VALUES ('Root');
INSERT INTO tb_level_1 (fk_level_0, value) VALUES (1, 'Level 1');
INSERT INTO tb_level_2 (fk_level_1, value) VALUES (1, 'Level 2');
INSERT INTO tb_level_3 (fk_level_2, value) VALUES (1, 'Level 3');
INSERT INTO tb_level_4 (fk_level_3, value) VALUES (1, 'Level 4');
INSERT INTO tb_level_5 (fk_level_4, value) VALUES (1, 'Level 5');

-- Test 1: Create shallow hierarchy (5 levels - should work)
\echo ''
\echo 'Test 1: Shallow hierarchy works (5 levels)'

CREATE TABLE tv_level_0 AS
SELECT
    pk_level_0,
    id,
    jsonb_build_object(
        'id', id::text,
        'value', value
    ) AS data
FROM tb_level_0;

CREATE TABLE tv_level_1 AS
SELECT
    l1.pk_level_1,
    l1.id,
    l1.fk_level_0,
    tv_level_0.id AS level_0_id,
    jsonb_build_object(
        'id', l1.id::text,
        'value', l1.value,
        'parent', tv_level_0.data
    ) AS data
FROM tb_level_1 l1
JOIN tv_level_0 ON tv_level_0.pk_level_0 = l1.fk_level_0;

CREATE TABLE tv_level_2 AS
SELECT
    l2.pk_level_2,
    l2.id,
    l2.fk_level_1,
    tv_level_1.id AS level_1_id,
    jsonb_build_object(
        'id', l2.id::text,
        'value', l2.value,
        'parent', tv_level_1.data
    ) AS data
FROM tb_level_2 l2
JOIN tv_level_1 ON tv_level_1.pk_level_1 = l2.fk_level_1;

CREATE TABLE tv_level_3 AS
SELECT
    l3.pk_level_3,
    l3.id,
    l3.fk_level_2,
    tv_level_2.id AS level_2_id,
    jsonb_build_object(
        'id', l3.id::text,
        'value', l3.value,
        'parent', tv_level_2.data
    ) AS data
FROM tb_level_3 l3
JOIN tv_level_2 ON tv_level_2.pk_level_2 = l3.fk_level_2;

CREATE TABLE tv_level_4 AS
SELECT
    l4.pk_level_4,
    l4.id,
    l4.fk_level_3,
    tv_level_3.id AS level_3_id,
    jsonb_build_object(
        'id', l4.id::text,
        'value', l4.value,
        'parent', tv_level_3.data
    ) AS data
FROM tb_level_4 l4
JOIN tv_level_3 ON tv_level_3.pk_level_3 = l4.fk_level_3;

CREATE TABLE tv_level_5 AS
SELECT
    l5.pk_level_5,
    l5.id,
    l5.fk_level_4,
    tv_level_4.id AS level_4_id,
    jsonb_build_object(
        'id', l5.id::text,
        'value', l5.value,
        'parent', tv_level_4.data
    ) AS data
FROM tb_level_5 l5
JOIN tv_level_4 ON tv_level_4.pk_level_4 = l5.fk_level_4;

SELECT assert_levels_fresh('creating the chain');
DO $$ BEGIN
    IF (SELECT COUNT(*) FROM pg_tview_meta) <> 6 THEN
        RAISE EXCEPTION 'FAIL: expected 6 TVIEWs in the chain';
    END IF;
END $$;

\echo '✓ Test 1 passed: 5-level hierarchy created successfully'

-- Test 2: Verify initial cascade works (within limit)
\echo ''
\echo 'Test 2: Verify cascade through 5 levels'

-- Update root (level_0)
UPDATE tb_level_0 SET value = 'Root Updated' WHERE pk_level_0 = 1;

-- Verify cascade reached all 5 levels
SELECT data->>'value' FROM tv_level_0 WHERE pk_level_0 = 1;
-- Expected: 'Root Updated'

SELECT data->'parent'->>'value' FROM tv_level_1 WHERE pk_level_1 = 1;
-- Expected: 'Root Updated'

SELECT data->'parent'->'parent'->>'value' FROM tv_level_2 WHERE pk_level_2 = 1;
-- Expected: 'Root Updated'

SELECT data->'parent'->'parent'->'parent'->>'value' FROM tv_level_3 WHERE pk_level_3 = 1;
-- Expected: 'Root Updated'

SELECT data->'parent'->'parent'->'parent'->'parent'->>'value' FROM tv_level_4 WHERE pk_level_4 = 1;
-- Expected: 'Root Updated'

SELECT data->'parent'->'parent'->'parent'->'parent'->'parent'->>'value' FROM tv_level_5 WHERE pk_level_5 = 1;
-- Expected: 'Root Updated'

SELECT assert_levels_fresh('an UPDATE of the root');
DO $$ BEGIN
    IF (SELECT data #>> '{parent,parent,parent,parent,parent,value}' FROM tv_level_5 WHERE pk_level_5 = 1)
       IS DISTINCT FROM 'Root Updated' THEN
        RAISE EXCEPTION 'FAIL: root update did not reach level 5';
    END IF;
END $$;

\echo '✓ Test 2 passed: Cascade propagated through 5 levels'

-- Test 3: Verify depth counter increments correctly
\echo ''
\echo 'Test 3: Verify depth tracking'

-- Update mid-level (level_2)
UPDATE tb_level_2 SET value = 'Level 2 Updated' WHERE pk_level_2 = 1;

-- Should cascade to level_3, level_4, level_5 (3 levels)
SELECT data->>'value' FROM tv_level_2 WHERE pk_level_2 = 1;
-- Expected: 'Level 2 Updated'

SELECT data->'parent'->>'value' FROM tv_level_3 WHERE pk_level_3 = 1;
-- Expected: 'Level 2 Updated'

SELECT assert_levels_fresh('an UPDATE of level 2');
DO $$ BEGIN
    IF (SELECT data->>'value' FROM tv_level_2 WHERE pk_level_2 = 1) IS DISTINCT FROM 'Level 2 Updated'
       OR (SELECT data #>> '{parent,parent,parent,value}' FROM tv_level_5 WHERE pk_level_5 = 1)
          IS DISTINCT FROM 'Level 2 Updated' THEN
        RAISE EXCEPTION 'FAIL: level 2 update did not reach levels 2..5';
    END IF;
    -- Levels above the write are not touched.
    IF (SELECT data #>> '{parent,value}' FROM tv_level_1 WHERE pk_level_1 = 1) IS DISTINCT FROM 'Root Updated' THEN
        RAISE EXCEPTION 'FAIL: level 1 changed by a level 2 update';
    END IF;
END $$;

\echo '✓ Test 3 passed: Depth tracking works'

-- Test 4: Depth limit enforcement
-- Rather than build a chain longer than the default limit, lower the limit
-- below this chain's depth.
\echo ''
\echo 'Test 4: Depth limit enforcement'

-- We've created 6 levels (0-5), which is within the default limit.
-- The depth limit is pg_tviews.max_propagation_depth.
\echo 'Verifying cascade depth limit configuration...'

-- Every level is registered
SELECT
    COUNT(*) > 0 AS has_depth_limit
FROM pg_tview_meta;
-- Expected: true (metadata exists)

-- The default limit leaves room for this chain (a root write takes one
-- propagation pass per level).
DO $$ BEGIN
    IF current_setting('pg_tviews.max_propagation_depth')::int < 6 THEN
        RAISE EXCEPTION 'FAIL: default max_propagation_depth below the 6-level chain';
    END IF;
END $$;

-- Below the chain's depth, a root write is refused rather than leaving the
-- lower levels stale.
SET pg_tviews.max_propagation_depth = 3;
DO $$
DECLARE refused text;
BEGIN
    BEGIN
        UPDATE tb_level_0 SET value = 'Too Deep' WHERE pk_level_0 = 1;
    EXCEPTION WHEN OTHERS THEN
        refused := SQLERRM;
    END;
    IF refused IS NULL OR refused NOT LIKE '%depth%' THEN
        RAISE EXCEPTION 'FAIL: a 6-level cascade was not refused at depth 3: %', refused;
    END IF;
END $$;
RESET pg_tviews.max_propagation_depth;

SELECT assert_levels_fresh('a write refused by the depth limit');
DO $$ BEGIN
    IF (SELECT value FROM tb_level_0 WHERE pk_level_0 = 1) IS DISTINCT FROM 'Root Updated' THEN
        RAISE EXCEPTION 'FAIL: the refused write changed tb_level_0';
    END IF;
END $$;

\echo '✓ Test 4 passed: Depth limit enforced'

-- Test 5: Verify cascade stops at appropriate depth
\echo ''
\echo 'Test 5: Verify cascade counts'

-- Count total cascades that happened for root update
-- (This is indirect - we verify all levels were updated)
SELECT COUNT(*) AS tview_count FROM pg_tview_meta;
-- Expected: 6 (levels 0-5)

DO $$ BEGIN
    IF (SELECT COUNT(*) FROM pg_tview_meta) <> 6 THEN
        RAISE EXCEPTION 'FAIL: expected 6 TVIEWs';
    END IF;
END $$;

\echo '✓ Test 5 passed: Cascade depth tracking correct'

-- Test 6: Performance check - deep cascade should still be fast
\echo ''
\echo 'Test 6: Performance check'

-- Time a cascade through all 5 levels
\timing on
UPDATE tb_level_0 SET value = 'Root Performance Test' WHERE pk_level_0 = 1;
\timing off

-- Verify update propagated
SELECT data->'parent'->'parent'->'parent'->'parent'->'parent'->>'value'
FROM tv_level_5
WHERE pk_level_5 = 1;
-- Expected: 'Root Performance Test'

SELECT assert_levels_fresh('a second UPDATE of the root');
DO $$ BEGIN
    IF (SELECT data #>> '{parent,parent,parent,parent,parent,value}' FROM tv_level_5 WHERE pk_level_5 = 1)
       IS DISTINCT FROM 'Root Performance Test' THEN
        RAISE EXCEPTION 'FAIL: second root update did not reach level 5';
    END IF;
END $$;

\echo '✓ Test 6 passed: Deep cascade completed'

\echo ''
\echo '=========================================='
\echo 'Test 43: All tests passed! ✓'
\echo '=========================================='

ROLLBACK;

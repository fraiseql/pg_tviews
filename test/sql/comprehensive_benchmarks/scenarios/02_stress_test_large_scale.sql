-- Large-Scale Stress Tests for pg_tviews
-- Tests performance and scalability with large datasets
-- Run with: psql -d test_db -f test/sql/comprehensive_benchmarks/scenarios/02_stress_test_large_scale.sql

-- Enable timing
\timing on

-- Clean up from previous runs
DROP TABLE IF EXISTS tv_stress_wide_10;
DROP TABLE IF EXISTS tv_stress_wide_9;
DROP TABLE IF EXISTS tv_stress_wide_8;
DROP TABLE IF EXISTS tv_stress_wide_7;
DROP TABLE IF EXISTS tv_stress_wide_6;
DROP TABLE IF EXISTS tv_stress_wide_5;
DROP TABLE IF EXISTS tv_stress_wide_4;
DROP TABLE IF EXISTS tv_stress_wide_3;
DROP TABLE IF EXISTS tv_stress_wide_2;
DROP TABLE IF EXISTS tv_stress_wide_1;
DROP TABLE IF EXISTS tv_stress_deep_5;
DROP TABLE IF EXISTS tv_stress_deep_4;
DROP TABLE IF EXISTS tv_stress_deep_3;
DROP TABLE IF EXISTS tv_stress_deep_2;
DROP TABLE IF EXISTS tv_stress_deep_1;
DROP TABLE IF EXISTS tv_stress_item;
DROP TABLE IF EXISTS tb_stress_item;
DROP TABLE IF EXISTS tb_stress_wide;
DROP TABLE IF EXISTS tb_stress_deep_base;

-- Ensure extension is loaded
CREATE EXTENSION IF NOT EXISTS pg_tviews;

-- ========================================
-- SCENARIO 1: Large Dataset (1M rows, single TVIEW)
-- ========================================

SELECT '=== SCENARIO 1: Large Dataset (1M rows) ===' as scenario;

-- Create base table with 1M rows
CREATE TABLE tb_stress_item (
    pk_item BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    fk_category INTEGER,
    value INTEGER,
    data_field TEXT
);

-- Generate 1M rows using generate_series (faster than individual inserts)
INSERT INTO tb_stress_item (fk_category, value, data_field)
SELECT
    (random() * 100)::INTEGER + 1,  -- categories 1-100
    (random() * 1000000)::INTEGER,  -- random values
    'data_' || gs::TEXT              -- data field
FROM generate_series(1, 1000000) gs;

-- Verify data
SELECT COUNT(*) as total_rows FROM tb_stress_item;
SELECT COUNT(DISTINCT fk_category) as categories FROM tb_stress_item;

-- Create TVIEW
SELECT 'Creating TVIEW on 1M rows...' as status;
CREATE TABLE tv_stress_item AS
SELECT
    tb_stress_item.pk_item,
    tb_stress_item.id,
    jsonb_build_object(
        'id', tb_stress_item.id,
        'categoryId', tb_stress_item.fk_category,
        'value', tb_stress_item.value
    ) as data
FROM tb_stress_item;

-- Measure initial creation time (reported by \timing)

-- Test single-row updates
SELECT 'Testing single-row updates...' as status;
UPDATE tb_stress_item
SET value = tb_stress_item.value + 1
WHERE tb_stress_item.fk_category = 1
LIMIT 1;

-- Test bulk updates (1K rows)
SELECT 'Testing bulk updates (1K rows)...' as status;
UPDATE tb_stress_item
SET value = tb_stress_item.value + 1
WHERE tb_stress_item.fk_category = 2
LIMIT 1000;

-- Test bulk updates (10K rows)
SELECT 'Testing bulk updates (10K rows)...' as status;
UPDATE tb_stress_item
SET value = tb_stress_item.value + 1
WHERE tb_stress_item.fk_category = 3
LIMIT 10000;

-- Test category-wide updates (affects many rows)
SELECT 'Testing category-wide updates...' as status;
UPDATE tb_stress_item
SET value = tb_stress_item.value + 1
WHERE tb_stress_item.fk_category = 4;

-- ========================================
-- SCENARIO 2: Deep Cascade (5 levels, 100K rows each)
-- ========================================

SELECT '=== SCENARIO 2: Deep Cascade (5 levels) ===' as scenario;

-- Create base table with 100K rows
CREATE TABLE tb_stress_deep_base (
    pk_base BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    level1_data TEXT,
    level2_data TEXT,
    level3_data TEXT,
    level4_data TEXT,
    level5_data TEXT
);

INSERT INTO tb_stress_deep_base (level1_data, level2_data, level3_data, level4_data, level5_data)
SELECT
    'level1_' || gs::TEXT,
    'level2_' || gs::TEXT,
    'level3_' || gs::TEXT,
    'level4_' || gs::TEXT,
    'level5_' || gs::TEXT
FROM generate_series(1, 100000) gs;

-- Level 1: Basic transformation
SELECT 'Creating Level 1 TVIEW...' as status;
CREATE TABLE tv_stress_deep_1 AS
SELECT
    tb_stress_deep_base.pk_base,
    tb_stress_deep_base.id,
    jsonb_build_object(
        'id', tb_stress_deep_base.id,
        'level1', tb_stress_deep_base.level1_data,
        'processed', true
    ) as data
FROM tb_stress_deep_base;

-- Level 2: Aggregation
SELECT 'Creating Level 2 TVIEW...' as status;
CREATE TABLE tv_stress_deep_2 AS
SELECT
    tv_stress_deep_1.pk_base,
    tv_stress_deep_1.id,
    jsonb_build_object(
        'id', tv_stress_deep_1.id,
        'level1', tv_stress_deep_1.data->>'level1',
        'level2', 'aggregated_' || tv_stress_deep_1.data->>'level1',
        'processed', true
    ) as data
FROM tv_stress_deep_1;

-- Level 3: Further transformation
SELECT 'Creating Level 3 TVIEW...' as status;
CREATE TABLE tv_stress_deep_3 AS
SELECT
    tv_stress_deep_2.pk_base,
    tv_stress_deep_2.id,
    jsonb_build_object(
        'id', tv_stress_deep_2.id,
        'level3', 'transformed_' || tv_stress_deep_2.data->>'level2',
        'chain', jsonb_build_array(
            tv_stress_deep_2.data->>'level1',
            tv_stress_deep_2.data->>'level2'
        ),
        'processed', true
    ) as data
FROM tv_stress_deep_2;

-- Level 4: Complex computation
SELECT 'Creating Level 4 TVIEW...' as status;
CREATE TABLE tv_stress_deep_4 AS
SELECT
    tv_stress_deep_3.pk_base,
    tv_stress_deep_3.id,
    jsonb_build_object(
        'id', tv_stress_deep_3.id,
        'level4', 'computed_' || length(tv_stress_deep_3.data->>'level3')::TEXT,
        'chain_length', jsonb_array_length(tv_stress_deep_3.data->'chain'),
        'processed', true
    ) as data
FROM tv_stress_deep_3;

-- Level 5: Final aggregation
SELECT 'Creating Level 5 TVIEW...' as status;
CREATE TABLE tv_stress_deep_5 AS
SELECT
    tv_stress_deep_4.pk_base,
    tv_stress_deep_4.id,
    jsonb_build_object(
        'id', tv_stress_deep_4.id,
        'level5', 'final_' || tv_stress_deep_4.data->>'level4',
        'total_chain_length', tv_stress_deep_4.data->>'chain_length',
        'completed', true
    ) as data
FROM tv_stress_deep_4;

-- Test cascade update (update base table, should propagate through all levels)
SELECT 'Testing deep cascade update...' as status;
UPDATE tb_stress_deep_base
SET level1_data = 'updated_' || level1_data
WHERE pk_base <= 100;  -- Update first 100 rows

-- ========================================
-- SCENARIO 3: Wide Cascade (1 base table → 10 TVIEWs)
-- ========================================

SELECT '=== SCENARIO 3: Wide Cascade (10 TVIEWs) ===' as scenario;

-- Create base table with 100K rows
CREATE TABLE tb_stress_wide (
    pk_wide BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    data1 TEXT,
    data2 TEXT,
    data3 TEXT,
    data4 TEXT,
    data5 TEXT,
    data6 TEXT,
    data7 TEXT,
    data8 TEXT,
    data9 TEXT,
    data10 TEXT
);

INSERT INTO tb_stress_wide (data1, data2, data3, data4, data5, data6, data7, data8, data9, data10)
SELECT
    'data1_' || gs::TEXT,
    'data2_' || gs::TEXT,
    'data3_' || gs::TEXT,
    'data4_' || gs::TEXT,
    'data5_' || gs::TEXT,
    'data6_' || gs::TEXT,
    'data7_' || gs::TEXT,
    'data8_' || gs::TEXT,
    'data9_' || gs::TEXT,
    'data10_' || gs::TEXT
FROM generate_series(1, 100000) gs;

-- Create 10 different TVIEWs from the same base table
SELECT 'Creating 10 wide TVIEWs...' as status;

CREATE TABLE tv_stress_wide_1 AS
SELECT pk_wide, id, jsonb_build_object('id', id, 'field', data1) as data FROM tb_stress_wide;

CREATE TABLE tv_stress_wide_2 AS
SELECT pk_wide, id, jsonb_build_object('id', id, 'field', data2) as data FROM tb_stress_wide;

CREATE TABLE tv_stress_wide_3 AS
SELECT pk_wide, id, jsonb_build_object('id', id, 'field', data3) as data FROM tb_stress_wide;

CREATE TABLE tv_stress_wide_4 AS
SELECT pk_wide, id, jsonb_build_object('id', id, 'field', data4) as data FROM tb_stress_wide;

CREATE TABLE tv_stress_wide_5 AS
SELECT pk_wide, id, jsonb_build_object('id', id, 'field', data5) as data FROM tb_stress_wide;

CREATE TABLE tv_stress_wide_6 AS
SELECT pk_wide, id, jsonb_build_object('id', id, 'field', data6) as data FROM tb_stress_wide;

CREATE TABLE tv_stress_wide_7 AS
SELECT pk_wide, id, jsonb_build_object('id', id, 'field', data7) as data FROM tb_stress_wide;

CREATE TABLE tv_stress_wide_8 AS
SELECT pk_wide, id, jsonb_build_object('id', id, 'field', data8) as data FROM tb_stress_wide;

CREATE TABLE tv_stress_wide_9 AS
SELECT pk_wide, id, jsonb_build_object('id', id, 'field', data9) as data FROM tb_stress_wide;

CREATE TABLE tv_stress_wide_10 AS
SELECT pk_wide, id, jsonb_build_object('id', id, 'field', data10) as data FROM tb_stress_wide;

-- Test wide cascade (update base table, affects all 10 TVIEWs)
SELECT 'Testing wide cascade update...' as status;
UPDATE tb_stress_wide
SET data1 = 'updated_' || data1
WHERE pk_wide <= 1000;  -- Update first 1000 rows

-- ========================================
-- MEMORY AND PERFORMANCE MONITORING
-- ========================================

SELECT '=== MEMORY AND PERFORMANCE MONITORING ===' as scenario;

-- Check memory usage during operations
SELECT 'Memory usage check:' as status;
SELECT
    pg_size_pretty(pg_total_relation_size('tb_stress_item')) as base_table_size,
    pg_size_pretty(pg_total_relation_size('tv_stress_item')) as tview_size,
    pg_size_pretty(pg_total_relation_size('tb_stress_deep_base')) as deep_base_size,
    pg_size_pretty(pg_total_relation_size('tv_stress_deep_5')) as deep_final_size;

-- Check index usage
SELECT 'Index analysis:' as status;
SELECT
    schemaname,
    tablename,
    indexname,
    pg_size_pretty(pg_relation_size(indexrelid)) as index_size
FROM pg_stat_user_indexes
WHERE schemaname = 'public'
  AND tablename LIKE 'tv_stress%'
ORDER BY pg_relation_size(indexrelid) DESC
LIMIT 10;

-- Performance summary
SELECT 'Performance summary:' as status;
SELECT
    'Large dataset' as scenario,
    COUNT(*) as rows_in_base,
    pg_size_pretty(pg_total_relation_size('tb_stress_item')) as base_size,
    pg_size_pretty(pg_total_relation_size('tv_stress_item')) as tview_size
FROM tb_stress_item
UNION ALL
SELECT
    'Deep cascade' as scenario,
    COUNT(*) as rows_in_base,
    pg_size_pretty(pg_total_relation_size('tb_stress_deep_base')) as base_size,
    pg_size_pretty(pg_total_relation_size('tv_stress_deep_5')) as tview_size
FROM tb_stress_deep_base
UNION ALL
SELECT
    'Wide cascade' as scenario,
    COUNT(*) as rows_in_base,
    pg_size_pretty(pg_total_relation_size('tb_stress_wide')) as base_size,
    pg_size_pretty(pg_total_relation_size('tv_stress_wide_10')) as tview_size
FROM tb_stress_wide;

-- ========================================
-- CLEANUP
-- ========================================

SELECT '=== CLEANUP ===' as scenario;

-- Clean up large tables (optional - comment out if you want to keep for analysis)
DROP TABLE IF EXISTS tv_stress_wide_10;
DROP TABLE IF EXISTS tv_stress_wide_9;
DROP TABLE IF EXISTS tv_stress_wide_8;
DROP TABLE IF EXISTS tv_stress_wide_7;
DROP TABLE IF EXISTS tv_stress_wide_6;
DROP TABLE IF EXISTS tv_stress_wide_5;
DROP TABLE IF EXISTS tv_stress_wide_4;
DROP TABLE IF EXISTS tv_stress_wide_3;
DROP TABLE IF EXISTS tv_stress_wide_2;
DROP TABLE IF EXISTS tv_stress_wide_1;
DROP TABLE IF EXISTS tv_stress_deep_5;
DROP TABLE IF EXISTS tv_stress_deep_4;
DROP TABLE IF EXISTS tv_stress_deep_3;
DROP TABLE IF EXISTS tv_stress_deep_2;
DROP TABLE IF EXISTS tv_stress_deep_1;
DROP TABLE IF EXISTS tv_stress_item;
DROP TABLE IF EXISTS tb_stress_item;
DROP TABLE IF EXISTS tb_stress_wide;
DROP TABLE IF EXISTS tb_stress_deep_base;

SELECT 'Stress tests completed successfully' as result;

-- Disable timing
\timing off
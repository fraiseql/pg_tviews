-- test/sql/12_schema_inference_validation.sql
-- Test: Edge cases - missing required columns

BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Test 1: Missing pk_ column (should return NULL pk_column)
    SELECT pg_tviews_analyze_select($$
        SELECT id, name FROM tb_user
    $$) -> 'pk_column' IS NULL AS missing_pk_handled;

    -- Test 2: Missing data column (should return NULL data_column)
    SELECT pg_tviews_analyze_select($$
        SELECT pk_user, id, name FROM tb_user
    $$) -> 'data_column' IS NULL AS missing_data_handled;

    -- Test 3: No columns (should error gracefully)
    SELECT pg_tviews_analyze_select($$
        SELECT FROM tb_user
    $$) IS NOT NULL AS empty_select_handled;

ROLLBACK;
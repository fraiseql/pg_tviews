-- test/sql/10_schema_inference_simple.sql
-- Test: Infer schema from simple SELECT

BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Create test base table
    CREATE TABLE tb_test_entity (
        pk_test_entity INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
        name TEXT
    );

    -- Test: Analyze simple SELECT (not creating TVIEW yet, just analyzing)
    SELECT jsonb_pretty(
        pg_tviews_analyze_select($$
            SELECT
                pk_test_entity,
                id,
                jsonb_build_object('id', id, 'name', name) AS data
            FROM tb_test_entity
        $$)
    );

ROLLBACK;
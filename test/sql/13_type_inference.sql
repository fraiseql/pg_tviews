-- test/sql/13_type_inference.sql
-- Test: Infer column types from PostgreSQL catalog

BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Create test table with various types
    CREATE TABLE tb_test_types (
        pk_test INTEGER PRIMARY KEY,
        id UUID NOT NULL,
        name TEXT,
        is_active BOOLEAN,
        created_at TIMESTAMPTZ DEFAULT NOW(),
        tags TEXT[],
        data JSONB
    );

    -- Test: Infer column types
    SELECT jsonb_pretty(
        pg_tviews_infer_types('tb_test_types', ARRAY[
            'pk_test',
            'id',
            'name',
            'is_active',
            'created_at',
            'tags',
            'data'
        ])
    );

ROLLBACK;
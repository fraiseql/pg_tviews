-- auto_convert_tviews.sql
-- Automatically detect tv_* tables and convert them to TVIEWs
--
-- This function scans for all tables matching the pattern tv_<entity>
-- and automatically:
-- 1. Creates a backing view v_<entity> from tb_<entity> table
-- 2. Calls pg_tviews_create() to register the TVIEW
--
-- Usage:
--   SELECT pg_tviews_auto_convert();
--   -- or for specific schema:
--   SELECT pg_tviews_auto_convert('public');

CREATE OR REPLACE FUNCTION pg_tviews_auto_convert(schema_name text DEFAULT 'public')
RETURNS TABLE (entity text, status text, message text) AS $$
DECLARE
    v_entity text;
    v_tb_table text;
    v_tv_table text;
    v_view_name text;
    v_select_sql text;
    v_columns text;
    v_pk_col text;
    v_result text;
    v_rec record;
BEGIN
    -- Iterate through all tv_* tables in the schema
    FOR v_rec IN
        SELECT tablename
        FROM pg_tables
        WHERE schemaname = schema_name
          AND tablename ~ '^tv_'
        ORDER BY tablename
    LOOP
        v_tv_table := v_rec.tablename;

        -- Extract entity name (remove 'tv_' prefix)
        v_entity := substring(v_tv_table from 4);
        v_tb_table := 'tb_' || v_entity;
        v_view_name := 'v_' || v_entity;
        v_pk_col := 'pk_' || v_entity;

        -- Check if base table exists
        IF NOT EXISTS (
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = schema_name
              AND table_name = v_tb_table
        ) THEN
            entity := v_entity;
            status := 'SKIP';
            message := format('Base table %s.%s not found', schema_name, v_tb_table);
            RETURN NEXT;
            CONTINUE;
        END IF;

        -- Get column list from base table (excluding system columns)
        SELECT string_agg(attname, ', ' ORDER BY attnum)
        INTO v_columns
        FROM pg_attribute
        WHERE attrelid = format('%s.%s', schema_name, v_tb_table)::regclass
          AND attnum > 0
          AND NOT attisdropped;

        IF v_columns IS NULL THEN
            entity := v_entity;
            status := 'SKIP';
            message := 'No columns found in base table';
            RETURN NEXT;
            CONTINUE;
        END IF;

        -- Drop existing view if it exists
        EXECUTE format('DROP VIEW IF EXISTS %I.%I', schema_name, v_view_name);

        -- Create backing view with all columns from base table
        v_select_sql := format(
            'SELECT %I, %s FROM %I.%I',
            v_pk_col,
            v_columns,
            schema_name,
            v_tb_table
        );

        BEGIN
            EXECUTE format(
                'CREATE VIEW %I.%I AS %s',
                schema_name,
                v_view_name,
                v_select_sql
            );

            -- Call pg_tviews_create to register the TVIEW
            v_result := pg_tviews_create(v_entity, v_select_sql);

            entity := v_entity;
            status := 'SUCCESS';
            message := format('TVIEW registered: %s', v_result);
            RETURN NEXT;
        EXCEPTION WHEN OTHERS THEN
            entity := v_entity;
            status := 'ERROR';
            message := format('Failed to create TVIEW: %s', SQLERRM);
            RETURN NEXT;
        END;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- Alternative: simpler variant that just reports what would be created
CREATE OR REPLACE FUNCTION pg_tviews_auto_convert_plan(schema_name text DEFAULT 'public')
RETURNS TABLE (entity text, base_table text, backing_view text, select_sql text) AS $$
DECLARE
    v_entity text;
    v_tb_table text;
    v_view_name text;
    v_select_sql text;
    v_columns text;
    v_pk_col text;
    v_rec record;
    v_tv_table text;
BEGIN
    FOR v_rec IN
        SELECT tablename
        FROM pg_tables
        WHERE schemaname = schema_name
          AND tablename ~ '^tv_'
        ORDER BY tablename
    LOOP
        v_tv_table := v_rec.tablename;
        v_entity := substring(v_tv_table from 4);
        v_tb_table := 'tb_' || v_entity;
        v_view_name := 'v_' || v_entity;
        v_pk_col := 'pk_' || v_entity;

        IF NOT EXISTS (
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = schema_name
              AND table_name = v_tb_table
        ) THEN
            CONTINUE;
        END IF;

        SELECT string_agg(attname, ', ' ORDER BY attnum)
        INTO v_columns
        FROM pg_attribute
        WHERE attrelid = format('%s.%s', schema_name, v_tb_table)::regclass
          AND attnum > 0
          AND NOT attisdropped;

        IF v_columns IS NULL THEN
            CONTINUE;
        END IF;

        v_select_sql := format(
            'SELECT %I, %s FROM %I.%I',
            v_pk_col,
            v_columns,
            schema_name,
            v_tb_table
        );

        entity := v_entity;
        base_table := format('%s.%s', schema_name, v_tb_table);
        backing_view := format('%s.%s', schema_name, v_view_name);
        select_sql := v_select_sql;
        RETURN NEXT;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

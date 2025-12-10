-- Phase 9A: Statement-Level Triggers
-- Installs statement-level triggers that fire once per statement instead of once per row

-- Function to install statement-level triggers for all TVIEW-managed tables
CREATE OR REPLACE FUNCTION pg_tviews_install_stmt_triggers()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    rec RECORD;
BEGIN
    -- Drop existing statement-level triggers first
    FOR rec IN
        SELECT
            tgname::text as trigger_name,
            tgrelid::regclass::text as table_name
        FROM pg_trigger
        WHERE tgname LIKE 'pg_tview_stmt_trigger'
    LOOP
        EXECUTE format('DROP TRIGGER IF EXISTS %I ON %s',
                      rec.trigger_name, rec.table_name);
    END LOOP;

    -- Install new statement-level triggers for all TVIEW-managed tables
    FOR rec IN
        SELECT DISTINCT
            m.table_oid,
            c.relname::text as table_name,
            m.entity
        FROM pg_tview_meta m
        JOIN pg_class c ON c.oid = m.table_oid
        WHERE c.relkind = 'r'  -- regular table
    LOOP
        -- Create statement-level trigger with transition tables
        EXECUTE format(
            'CREATE TRIGGER pg_tview_stmt_trigger
             AFTER INSERT OR UPDATE OR DELETE ON %s
             REFERENCING OLD TABLE AS old_table NEW TABLE AS new_table
             FOR EACH STATEMENT
             EXECUTE FUNCTION pg_tview_stmt_trigger_handler()',
            rec.table_name
        );

        RAISE NOTICE 'Installed statement-level trigger on table: %', rec.table_name;
    END LOOP;

    RAISE NOTICE 'Statement-level triggers installed for all TVIEW-managed tables';
END;
$$;

-- Function to uninstall statement-level triggers
CREATE OR REPLACE FUNCTION pg_tviews_uninstall_stmt_triggers()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    rec RECORD;
BEGIN
    FOR rec IN
        SELECT
            tgname::text as trigger_name,
            tgrelid::regclass::text as table_name
        FROM pg_trigger
        WHERE tgname LIKE 'pg_tview_stmt_trigger'
    LOOP
        EXECUTE format('DROP TRIGGER IF EXISTS %I ON %s',
                      rec.trigger_name, rec.table_name);
        RAISE NOTICE 'Dropped statement-level trigger on table: %', rec.table_name;
    END LOOP;

    RAISE NOTICE 'All statement-level triggers uninstalled';
END;
$$;

-- Install statement-level triggers by default
-- This will be called during extension setup
SELECT pg_tviews_install_stmt_triggers();
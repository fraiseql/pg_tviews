-- Event trigger for CREATE TABLE interception
-- Fires AFTER the table is created, providing safe SPI context

CREATE OR REPLACE FUNCTION pg_tviews_handle_ddl_event()
RETURNS event_trigger
LANGUAGE plpgsql
AS $$
DECLARE
    obj record;
BEGIN
    -- Loop through all objects created by this DDL command
    FOR obj IN SELECT * FROM pg_event_trigger_ddl_commands()
    LOOP
        -- Log for debugging
        RAISE INFO 'pg_tviews: DDL event - command_tag=%, object_type=%, object_identity=%',
            obj.command_tag, obj.object_type, obj.object_identity;

        -- Only process CREATE TABLE and SELECT INTO
        IF obj.command_tag IN ('CREATE TABLE', 'CREATE TABLE AS', 'SELECT INTO') THEN
            -- Check if table name starts with tv_
            IF obj.object_identity LIKE 'public.tv_%' OR obj.object_identity LIKE 'tv_%' THEN
                RAISE INFO 'pg_tviews: Detected TVIEW creation: %', obj.object_identity;

                -- Extract table name (remove schema prefix if present)
                DECLARE
                    table_name_only TEXT;
                BEGIN
                    table_name_only := CASE
                        WHEN obj.object_identity LIKE '%.%'
                        THEN split_part(obj.object_identity, '.', 2)
                        ELSE obj.object_identity
                    END;

                    RAISE INFO 'pg_tviews: Converting table ''%'' to TVIEW', table_name_only;

                    -- Call Rust conversion function
                    -- This runs in safe SPI context (after DDL completed)
                    PERFORM pg_tviews_convert_table(table_name_only);

                    RAISE INFO 'pg_tviews: Successfully converted ''%'' to TVIEW', table_name_only;
                EXCEPTION
                    WHEN OTHERS THEN
                        -- Log error but don't fail the transaction
                        -- The table was already created by PostgreSQL
                        RAISE WARNING 'pg_tviews: Failed to convert ''%'' to TVIEW: %',
                            table_name_only, SQLERRM;
                        RAISE WARNING 'pg_tviews: Table exists as regular table, not a TVIEW';
                END;
            END IF;
        END IF;
    END LOOP;
END;
$$;

-- Create the event trigger
DROP EVENT TRIGGER IF EXISTS pg_tviews_ddl_end;
CREATE EVENT TRIGGER pg_tviews_ddl_end
    ON ddl_command_end
    WHEN TAG IN ('CREATE TABLE', 'CREATE TABLE AS', 'SELECT INTO')
    EXECUTE FUNCTION pg_tviews_handle_ddl_event();

-- Add comment
COMMENT ON EVENT TRIGGER pg_tviews_ddl_end IS
'Intercepts CREATE TABLE tv_* commands and converts them to TVIEWs';
-- pg_tviews_time_boundary(): the time refresh of every time-dependent TVIEW the
-- caller owns, each in its own transaction, dependencies first: a writer waits at
-- most for one TVIEW's rows, and each TVIEW's boundary is recorded as it commits.
-- A procedure, so it must be CALLed outside a transaction block. It has no SET
-- search_path: a procedure with a SET clause cannot COMMIT. It runs as its caller,
-- and names every object with its schema.
CREATE PROCEDURE @extschema@.pg_tviews_time_boundary()
LANGUAGE plpgsql
AS $$
DECLARE
    next_tview TEXT;
BEGIN
    FOR next_tview IN
        WITH RECURSIVE edges(entity, dependency) AS (
            SELECT DISTINCT r.entity, m.entity
            FROM @extschema@.reads r
            JOIN @extschema@.pg_tview_meta m
              ON r.relid IN (m.view_oid::oid, m.table_oid::oid)
            WHERE m.entity <> r.entity
        ),
        depth(entity, level) AS (
            SELECT m.entity, 0 FROM @extschema@.pg_tview_meta m
          UNION
            SELECT e.entity, d.level + 1
            FROM depth d JOIN edges e ON e.dependency = d.entity
            WHERE d.level < 100
        )
        SELECT pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname)
        FROM depth d
        JOIN @extschema@.pg_tview_meta m ON m.entity = d.entity
        JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
        JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
        WHERE m.time_dependent AND pg_catalog.pg_has_role(c.relowner, 'USAGE')
        GROUP BY d.entity, n.nspname, c.relname
        ORDER BY max(d.level), d.entity
    LOOP
        PERFORM @extschema@.pg_tviews_refresh_time_dependent(next_tview);
        COMMIT;
    END LOOP;
END;
$$;

COMMENT ON PROCEDURE @extschema@.pg_tviews_time_boundary() IS
'Refresh every time-dependent TVIEW the caller owns at the time boundary, one transaction each, dependencies first';

-- The time refresh at the boundary, one transaction per TVIEW, dependencies first:
-- a writer waits at most for one TVIEW's rows, and each TVIEW's boundary is recorded
-- as it commits. Procedures, so they must be CALLed outside a transaction block.
-- They have no SET search_path, as a procedure with a SET clause cannot COMMIT: they
-- run as their caller and name every object with its schema.

-- Every time-dependent TVIEW, dependencies first (an entity after every TVIEW its
-- backing view reads), and whether the caller may refresh it as its owner.
CREATE FUNCTION @extschema@.pg_tviews_time_dependent_order()
RETURNS TABLE (tview TEXT, owned BOOLEAN)
LANGUAGE sql STABLE
SET search_path = pg_catalog, pg_temp
AS $$
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
    SELECT pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname),
           pg_catalog.pg_has_role(c.relowner, 'USAGE')
    FROM depth d
    JOIN @extschema@.pg_tview_meta m ON m.entity = d.entity
    JOIN pg_catalog.pg_class c ON c.oid = m.table_oid
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
    WHERE m.time_dependent
    GROUP BY d.entity, n.nspname, c.relname, c.relowner
    ORDER BY max(d.level), d.entity
$$;

COMMENT ON FUNCTION @extschema@.pg_tviews_time_dependent_order() IS
'Internal: the time-dependent TVIEWs in dependency order, and whether the caller owns each';

-- Every time-dependent TVIEW the caller owns; the others are named in a WARNING.
CREATE PROCEDURE @extschema@.pg_tviews_time_boundary()
LANGUAGE plpgsql
AS $$
DECLARE
    next_tview TEXT;
    skipped TEXT;
BEGIN
    SELECT pg_catalog.string_agg(o.tview, ', ' ORDER BY o.tview) INTO skipped
    FROM @extschema@.pg_tviews_time_dependent_order() o WHERE NOT o.owned;
    FOR next_tview IN
        SELECT o.tview FROM @extschema@.pg_tviews_time_dependent_order() o WHERE o.owned
    LOOP
        PERFORM @extschema@.pg_tviews_refresh_time_dependent(next_tview);
        COMMIT;
    END LOOP;
    IF skipped IS NOT NULL THEN
        RAISE WARNING 'time-dependent TVIEWs not refreshed, owned by other roles: %', skipped
            USING HINT = 'Run it as their owner, or grant CALL tviews.pg_tviews_time_boundary_all() '
                         'to the role that runs it (docs/user-guides/operators.md).';
    END IF;
END;
$$;

COMMENT ON PROCEDURE @extschema@.pg_tviews_time_boundary() IS
'Refresh every time-dependent TVIEW the caller owns at the time boundary, one transaction each, dependencies first';

-- An operator's: every time-dependent TVIEW, each as its owner.
CREATE PROCEDURE @extschema@.pg_tviews_time_boundary_all()
LANGUAGE plpgsql
AS $$
DECLARE
    next_tview TEXT;
BEGIN
    FOR next_tview IN SELECT o.tview FROM @extschema@.pg_tviews_time_dependent_order() o
    LOOP
        PERFORM @extschema@.pg_tviews_refresh_time_dependent_all(next_tview);
        COMMIT;
    END LOOP;
END;
$$;

COMMENT ON PROCEDURE @extschema@.pg_tviews_time_boundary_all() IS
'Refresh every time-dependent TVIEW at the time boundary, each as its owner, one transaction each, dependencies first';

REVOKE EXECUTE ON PROCEDURE @extschema@.pg_tviews_time_boundary_all() FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_refresh_time_dependent_all(TEXT) FROM PUBLIC;

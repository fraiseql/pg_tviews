-- pg_tviews_reregister_all(): an operator's, like the functions in
-- `crate::api::maintenance`. Every TVIEW, dependencies first: an entity comes after every TVIEW its backing
-- view reads, through views. Each runs in its own subtransaction, so a failure
-- becomes that entity's status and the others go on; `strict` raises at the end.
CREATE FUNCTION @extschema@.pg_tviews_reregister_all(strict BOOLEAN DEFAULT false)
RETURNS TABLE (entity TEXT, status TEXT)
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
#variable_conflict use_column
DECLARE
    next_entity TEXT;
    failures INTEGER := 0;
    first_state TEXT;
    first_message TEXT;
BEGIN
    FOR next_entity IN
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
        SELECT d.entity FROM depth d GROUP BY d.entity ORDER BY max(d.level), d.entity
    LOOP
        entity := next_entity;
        BEGIN
            PERFORM @extschema@.pg_tviews_reregister(next_entity);
            status := 'reregistered';
        -- catches every error: it becomes the entity's status; strict raises the first.
        EXCEPTION WHEN OTHERS THEN
            status := SQLERRM;
            failures := failures + 1;
            first_state := COALESCE(first_state, SQLSTATE);
            first_message := COALESCE(first_message, 'tv_' || next_entity || ': ' || SQLERRM);
        END;
        RETURN NEXT;
    END LOOP;
    -- The first failure's SQLSTATE: a lock timeout or a deadlock stays retryable.
    IF strict AND failures > 0 THEN
        RAISE EXCEPTION 'pg_tviews: % TVIEW(s) could not be re-registered; first: %',
            failures, first_message
            USING ERRCODE = first_state,
                  HINT = 'SELECT * FROM tviews.pg_tviews_reregister_all() lists them';
    END IF;
END;
$$;

-- pg_tviews 0.1.0-beta.25 → 0.1.0-beta.26
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

-- Registration derives more (#187, #188, #189): a read under window functions
-- all partitioned by a linked column maps through the partition, UNION branches
-- keyed by their own tables (a column, or an expression of one row) get a root
-- each, and a materialized view read is uncascaded (refused under the error
-- policy). Re-derive every TVIEW with pg_tviews_reregister_all() after the update.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;

-- A TVIEW's table dropped by DROP OWNED (which names a role's objects directly)
-- is deregistered like one dropped as a dependent (#186).
CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_handle_drop_event()
RETURNS event_trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, @extschema@, pg_temp
AS $$
DECLARE
    entity_name TEXT;
BEGIN
    FOR entity_name IN
        SELECT DISTINCT m.entity
        FROM pg_catalog.pg_event_trigger_dropped_objects() AS d
        JOIN @extschema@.pg_tview_meta AS m
          ON d.objid IN (m.view_oid, m.table_oid)
        WHERE (NOT d.original OR TG_TAG = 'DROP OWNED')
          AND d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass
          AND d.objsubid = 0
    LOOP
        BEGIN
            PERFORM @extschema@.pg_tviews_handle_dropped(entity_name);
        EXCEPTION WHEN OTHERS THEN
            -- Never abort the user's DROP.
            RAISE WARNING 'pg_tviews: cleanup of tv_% after a dependent drop failed: %',
                entity_name, SQLERRM;
        END;
    END LOOP;
END;
$$;

-- Backing views beta.25 left behind when a TVIEW's table went with its schema
-- (DROP SCHEMA … CASCADE) or its owner's objects (#186): views named
-- <schema>__tv_<entity> in @extschema@ that no TVIEW owns block re-creating the
-- TVIEW. Dropped unless something depends on them.
DO $$
DECLARE
    r record;
BEGIN
    FOR r IN
        SELECT v.oid::pg_catalog.regclass AS view
        FROM pg_catalog.pg_class v
        WHERE v.relnamespace = '@extschema@'::pg_catalog.regnamespace
          AND v.relkind = 'v'
          AND v.relname LIKE '%\_\_tv\_%'
          AND NOT EXISTS (SELECT 1 FROM @extschema@.pg_tview_meta m
                          WHERE m.view_oid::pg_catalog.oid = v.oid)
          AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d
                          WHERE d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass
                            AND d.objid = v.oid AND d.deptype = 'e')
    LOOP
        BEGIN
            EXECUTE pg_catalog.format('DROP VIEW %s', r.view);
        EXCEPTION WHEN dependent_objects_still_exist THEN
            RAISE NOTICE 'pg_tviews: % belongs to no TVIEW but other objects depend on it; kept',
                r.view;
        END;
    END LOOP;
END $$;

-- The event trigger that reports a `CREATE TABLE tv_* AS` the ProcessUtility hook did
-- not intercept. PL/pgSQL because pgrx generates RETURNS VOID instead of the required
-- RETURNS event_trigger pseudo-type.
-- Event trigger handler: the ProcessUtility hook turns CREATE TABLE tv_* AS into a TVIEW
-- before PostgreSQL creates anything, so a tv_* table created this way means the hook did
-- not see the statement (pg_tviews is not in shared_preload_libraries): fail loudly
-- rather than leave a table deploy tools would take for a TVIEW.
CREATE FUNCTION @extschema@.pg_tviews_handle_ddl_event()
RETURNS event_trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    obj record;
BEGIN
    FOR obj IN SELECT * FROM pg_catalog.pg_event_trigger_ddl_commands()
    LOOP
        -- Only CTAS-style creation: a plain `CREATE TABLE tv_x (cols…)` has no query to
        -- make a TVIEW of, so it stays a plain table.
        IF obj.command_tag IN ('CREATE TABLE AS', 'SELECT INTO') THEN
            -- Only intercept tv_* tables
            IF obj.object_identity LIKE '%.tv_%' OR obj.object_identity LIKE 'tv_%' THEN
                RAISE EXCEPTION 'pg_tviews: cannot convert ''%'' to a TVIEW: the statement '
                                'was not intercepted', obj.object_identity
                    USING ERRCODE = 'object_not_in_prerequisite_state',
                          DETAIL = 'pg_tviews is not active in this session''s ProcessUtility '
                                   'hook',
                          HINT = 'Add pg_tviews to shared_preload_libraries in '
                                 'postgresql.conf and restart PostgreSQL, or create the TVIEW '
                                 'with tviews.pg_tviews_create_or_replace().';
            END IF;
        END IF;
    END LOOP;
END;
$$;

-- Create the event trigger (fires after CREATE TABLE completes — safe SPI context)
CREATE EVENT TRIGGER pg_tviews_ddl_end
    ON ddl_command_end
    WHEN TAG IN ('CREATE TABLE', 'CREATE TABLE AS', 'SELECT INTO')
    EXECUTE FUNCTION @extschema@.pg_tviews_handle_ddl_event();

COMMENT ON EVENT TRIGGER pg_tviews_ddl_end IS
'Fails a CREATE TABLE tv_* AS that the pg_tviews hook did not turn into a TVIEW';

-- Event trigger handler: deregister a TVIEW whose backing view or table was dropped as
-- a dependent of something else.  The base-table -> tview link is not
-- a hard PG dependency, so CASCADE from a base table, a helper view or a schema removes
-- the backing view v_* (and the base-table triggers on that table) but never the
-- trigger-populated tv_* table, its pg_tview_meta row or its triggers on other tables.
-- The dropped view is matched by OID, so any TVIEW reading the dropped object is found,
-- whatever its name.
--
-- Only objects dropped as dependents (original = false) count: pg_tviews' own drops of
-- v_* / tv_* (pg_tviews_drop, DROP TABLE tv_* via the ProcessUtility hook) name them
-- directly, so they never re-enter here. DROP OWNED names a role's objects directly
-- too, and pg_tviews never runs it: its drops count whatever their flag.
--
-- PL/pgSQL (not #[pg_extern]) because pgrx cannot emit RETURNS event_trigger.  It fires for
-- EVERY dropped object system-wide, so it must be cheap: references are schema-qualified
-- via @extschema@ (search-path independent). A cleanup that fails fails the DROP, with
-- its own SQLSTATE: committing the DROP would leave the TVIEW half deregistered.
--
-- Runs as the dropping role: PostgreSQL authorized that role's drop, and
-- pg_tviews_handle_dropped() does the rest without giving it more rights (see there).
CREATE FUNCTION @extschema@.pg_tviews_handle_drop_event()
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
        PERFORM @extschema@.pg_tviews_handle_dropped(entity_name);
    END LOOP;
END;
$$;

CREATE EVENT TRIGGER pg_tviews_sql_drop
    ON sql_drop
    EXECUTE FUNCTION @extschema@.pg_tviews_handle_drop_event();

COMMENT ON EVENT TRIGGER pg_tviews_sql_drop IS
'Deregisters a TVIEW whose backing view or table was dropped as a dependent';

-- Catalog rows loaded by pg_restore carry the source database's OIDs and column
-- numbers inside plan (view_oid / table_oid are regclass and re-resolve on their
-- own): each row inserted is rebound to the restored relations, found by their
-- qualified names, and columns, found by their names (pg_tviews_plan_rebind).
CREATE FUNCTION @extschema@.pg_tviews_meta_rebind()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
    NEW.plan := @extschema@.pg_tviews_plan_rebind(NEW.entity, NEW.plan);
    RETURN NEW;
END;
$$;

CREATE TRIGGER pg_tview_meta_rebind
    BEFORE INSERT ON @extschema@.pg_tview_meta
    FOR EACH ROW
    EXECUTE FUNCTION @extschema@.pg_tviews_meta_rebind();

-- Other backends cache TVIEW metadata. Any write to the catalog
-- invalidates its relcache entry at commit, which every backend watches.
CREATE FUNCTION @extschema@.pg_tviews_meta_changed()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
    PERFORM @extschema@.pg_tviews_invalidate_caches(TG_RELID);
    RETURN NULL;
END;
$$;

CREATE TRIGGER pg_tview_meta_changed
    AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON @extschema@.pg_tview_meta
    FOR EACH STATEMENT
    EXECUTE FUNCTION @extschema@.pg_tviews_meta_changed();

-- Maintenance acting on every TVIEW (the `_all` functions) is not for PUBLIC: an
-- operator role is granted it (docs/user-guides/operators.md). Each function
-- acting on, or showing, one TVIEW checks that the caller owns it.
REVOKE EXECUTE ON FUNCTION
    @extschema@.pg_tviews_refresh_all(),
    @extschema@.pg_tviews_rebuild_all(BOOLEAN),
    @extschema@.pg_tviews_reregister_all(BOOLEAN),
    @extschema@.pg_tviews_ensure_propagation_indexes_all(BOOLEAN),
    @extschema@.pg_tviews_reset_stats_all(),
    @extschema@.pg_tviews_invalidate_caches(OID)
FROM PUBLIC;

-- A value lock's tag is keyed with the server's secret: only a superuser may
-- compute one.
REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_value_lock_tag(REGCLASS, TEXT, TEXT) FROM PUBLIC;

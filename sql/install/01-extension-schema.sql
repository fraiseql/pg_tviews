-- The control file fixes the install schema to `tviews`. CREATE
-- EXTENSION creates it when missing, owned by the installing role, but adopts an
-- existing one as is: refuse one owned by another role, which could replace the
-- objects created in it. The install script never uses CREATE OR REPLACE or
-- IF NOT EXISTS, so an object planted under one of its names is an error too.
-- Supported PostgreSQL version: 18. Refuse older servers here, before
-- anything loads the library, with a message instead of a load error. The library
-- must be preloaded: its hooks, its shared statistics and its rebuild worker exist
-- only then, and there is no mode without them.
DO $$
BEGIN
    IF pg_catalog.current_setting('server_version_num')::int < 180000 THEN
        RAISE EXCEPTION 'pg_tviews requires PostgreSQL 18 or later (this server is %)',
            pg_catalog.current_setting('server_version');
    END IF;
    IF NOT pg_catalog.current_setting('shared_preload_libraries')
           OPERATOR(pg_catalog.~) '(^|,)\s*("?)(\$libdir/)?pg_tviews\2\s*(,|$)' THEN
        RAISE EXCEPTION 'pg_tviews is not in shared_preload_libraries'
            USING ERRCODE = 'object_not_in_prerequisite_state',
                  HINT = 'Add pg_tviews to shared_preload_libraries and restart the server.';
    END IF;
END
$$;

DO $$
DECLARE
    schema_owner NAME;
BEGIN
    SELECT pg_catalog.pg_get_userbyid(n.nspowner) INTO schema_owner
      FROM pg_catalog.pg_namespace n
     WHERE n.nspname = '@extschema@';
    IF schema_owner IS DISTINCT FROM CURRENT_USER THEN
        RAISE EXCEPTION 'schema "@extschema@" already exists and is owned by role "%"',
            schema_owner
            USING HINT = 'pg_tviews installs into schema @extschema@, which must be owned '
                         'by the role running CREATE EXTENSION. Drop the schema or change '
                         'its owner; to restore a dump, restore it as that owner or with '
                         'pg_restore --no-owner.';
    END IF;
END
$$;

-- Every role reaches the triggers, functions and catalog views.
GRANT USAGE ON SCHEMA @extschema@ TO PUBLIC;


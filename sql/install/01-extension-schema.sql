-- The control file fixes the install schema to `tviews`. CREATE
-- EXTENSION creates it when missing, owned by the installing role, but adopts an
-- existing one as is: refuse one owned by another role, which could replace the
-- objects created in it. The install script never uses CREATE OR REPLACE or
-- IF NOT EXISTS, so an object planted under one of its names is an error too.
-- Supported PostgreSQL versions: 16, 17, 18. Refuse older servers here, before
-- anything loads the library, with a message instead of a load error.
DO $$
BEGIN
    IF pg_catalog.current_setting('server_version_num')::int < 160000 THEN
        RAISE EXCEPTION 'pg_tviews requires PostgreSQL 16 or later (this server is %)',
            pg_catalog.current_setting('server_version');
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


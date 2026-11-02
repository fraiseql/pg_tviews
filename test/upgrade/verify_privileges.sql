-- Checks that hold once the extension is upgraded or migrated
-- (test/upgrade/upgrade_check.sh). Needs neither tviews nor public on search_path.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
SET search_path TO pg_catalog;

-- The readers of a TVIEW read its table. Its backing view is its owner's alone: it
-- reads the base tables with the owner's privileges, so the SELECT grants a previous
-- release copied onto it from the table (#181) are revoked by the update.
SET ROLE upgrade_schema_reader;
SELECT EXISTS (SELECT 1 FROM public.tv_user), EXISTS (SELECT 1 FROM public.tv_post) \gset
SET ROLE upgrade_table_reader;
SELECT EXISTS (SELECT 1 FROM public.tv_user) \gset
RESET ROLE;
DO $$
DECLARE
    granted text;
BEGIN
    SELECT pg_catalog.string_agg(DISTINCT v.oid::pg_catalog.regclass::text || ' to '
               || CASE WHEN a.grantee = 0 THEN 'PUBLIC' ELSE pg_catalog.pg_get_userbyid(a.grantee)::text END, ', ')
      INTO granted
    FROM tviews.pg_tview_meta m JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid,
         pg_catalog.aclexplode(v.relacl) a
    WHERE a.grantee <> v.relowner;
    IF granted IS NOT NULL THEN
        RAISE EXCEPTION 'upgrade check: backing views granted to other roles than their owner: %', granted;
    END IF;
END $$;

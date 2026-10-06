-- Checks that hold once the extension is upgraded or migrated
-- (test/upgrade/upgrade_check.sh). Needs neither tviews nor public on search_path.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
SET search_path TO pg_catalog;

-- A backing view's SELECT grants are its table's (#181): a role that reads a TVIEW
-- reads its backing view, as fraisier's deploy probe does, and no other role does.
SET ROLE upgrade_schema_reader;
SELECT EXISTS (SELECT 1 FROM public.tv_user), EXISTS (SELECT 1 FROM tviews.public__tv_user),
       EXISTS (SELECT 1 FROM public.tv_post), EXISTS (SELECT 1 FROM tviews.public__tv_post) \gset
SET ROLE upgrade_table_reader;
SELECT EXISTS (SELECT 1 FROM public.tv_user), EXISTS (SELECT 1 FROM tviews.public__tv_user) \gset
RESET ROLE;
DO $$ BEGIN
    IF pg_catalog.has_table_privilege('upgrade_table_reader', 'tviews.public__tv_post', 'SELECT') THEN
        RAISE EXCEPTION 'upgrade check: a role that cannot read tv_post reads its backing view';
    END IF;
END $$;

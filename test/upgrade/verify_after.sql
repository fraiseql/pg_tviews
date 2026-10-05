-- Checks that hold only once every TVIEW is re-registered after the upgrade
-- (test/upgrade/upgrade_check.sh). Needs neither tviews nor public on search_path.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
SET search_path TO pg_catalog;

-- A virtual generated column read through a join follows its inputs (#179).
SELECT pg_catalog.to_regclass('public.tv_holder') IS NOT NULL AS virtual_fixture \gset
\if :virtual_fixture
UPDATE public.tb_badge SET name = name || '+';
DO $$ BEGIN
    IF EXISTS ((SELECT pk_holder, data FROM public.tv_holder EXCEPT SELECT pk_holder, data FROM public.v_holder)
               UNION ALL
               (SELECT pk_holder, data FROM public.v_holder EXCEPT SELECT pk_holder, data FROM public.tv_holder)) THEN
        RAISE EXCEPTION 'upgrade check: tv_holder does not follow the input of its virtual column';
    END IF;
END $$;
\endif

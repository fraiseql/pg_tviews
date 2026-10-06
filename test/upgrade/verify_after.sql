-- Checks that hold only once every TVIEW is re-registered after the upgrade
-- (test/upgrade/upgrade_check.sh). Needs neither tviews nor public on search_path.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
SET search_path TO pg_catalog;

-- The backing views moved to tviews (#181): none is left in an application schema,
-- and the application can take the v_<entity> names.
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tviews.pg_tview_meta m
               JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid
               WHERE v.relnamespace <> 'tviews'::pg_catalog.regnamespace) THEN
        RAISE EXCEPTION 'upgrade check: a backing view is outside tviews';
    END IF;
    IF pg_catalog.to_regclass('tviews.public__tv_user') IS NULL
       OR pg_catalog.to_regclass('tviews.app__tv_note') IS NULL THEN
        RAISE EXCEPTION 'upgrade check: backing views not named <schema>__tv_<entity>';
    END IF;
END $$;
-- The fitted name the upgrade computed is the one pg_tviews derives: a rename of
-- the table back and forth leaves it as it is.
SELECT pg_catalog.to_regclass('public.tv_long_entity_name_for_the_upgrade_fitter_check_abcdefghij') IS NOT NULL AS long_fixture \gset
\if :long_fixture
CREATE TEMP TABLE upgraded_name AS
    SELECT v.relname::text AS name FROM tviews.pg_tview_meta m
    JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid
    WHERE m.entity = 'long_entity_name_for_the_upgrade_fitter_check_abcdefghij';
ALTER TABLE public.tv_long_entity_name_for_the_upgrade_fitter_check_abcdefghij RENAME TO tv_long_tmp;
ALTER TABLE public.tv_long_tmp RENAME TO tv_long_entity_name_for_the_upgrade_fitter_check_abcdefghij;
DO $$ BEGIN
    IF (SELECT name FROM upgraded_name) IS DISTINCT FROM
       (SELECT v.relname::text FROM tviews.pg_tview_meta m
        JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid
        WHERE m.entity = 'long_entity_name_for_the_upgrade_fitter_check_abcdefghij')
       OR pg_catalog.octet_length((SELECT name FROM upgraded_name)) <> 63 THEN
        RAISE EXCEPTION 'upgrade check: the upgrade fitted the long backing view name as %',
            (SELECT name FROM upgraded_name);
    END IF;
END $$;
\endif

CREATE VIEW public.v_user AS SELECT pk_user, name FROM public.tb_user;
UPDATE public.tb_user SET name = name || '#';
DO $$ BEGIN
    IF EXISTS ((SELECT pk_user, data FROM public.tv_user EXCEPT SELECT pk_user, data FROM tviews.public__tv_user)
               UNION ALL
               (SELECT pk_user, data FROM tviews.public__tv_user EXCEPT SELECT pk_user, data FROM public.tv_user)) THEN
        RAISE EXCEPTION 'upgrade check: tv_user does not follow tb_user with an application v_user';
    END IF;
END $$;
DROP VIEW public.v_user;

-- A rename of an ancestor refreshes the nodes whose path holds it (#182).
SELECT pg_catalog.to_regclass('public.tv_node') IS NOT NULL AS node_fixture \gset
\if :node_fixture
UPDATE public.tb_node SET name = name || '+' WHERE pk_node = 1;
DO $$ BEGIN
    IF EXISTS ((SELECT pk_node, data FROM public.tv_node EXCEPT SELECT pk_node, data FROM tviews.public__tv_node)
               UNION ALL
               (SELECT pk_node, data FROM tviews.public__tv_node EXCEPT SELECT pk_node, data FROM public.tv_node)) THEN
        RAISE EXCEPTION 'upgrade check: tv_node does not follow a rename of an ancestor';
    END IF;
END $$;
\endif

-- A virtual generated column read through a join follows its inputs (#179).
SELECT pg_catalog.to_regclass('public.tv_holder') IS NOT NULL AS virtual_fixture \gset
\if :virtual_fixture
UPDATE public.tb_badge SET name = name || '+';
DO $$ BEGIN
    IF EXISTS ((SELECT pk_holder, data FROM public.tv_holder EXCEPT SELECT pk_holder, data FROM tviews.public__tv_holder)
               UNION ALL
               (SELECT pk_holder, data FROM tviews.public__tv_holder EXCEPT SELECT pk_holder, data FROM public.tv_holder)) THEN
        RAISE EXCEPTION 'upgrade check: tv_holder does not follow the input of its virtual column';
    END IF;
END $$;
\endif

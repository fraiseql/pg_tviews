-- Write to every fixture base table (test/upgrade/fixtures.sql) and check that each
-- TVIEW equals its backing view. Needs neither tviews nor public on search_path.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
SET search_path TO pg_catalog;

UPDATE public.tb_user SET name = name || '+', bio = bio || '+';
INSERT INTO public.tb_user (pk_user, name) VALUES ((SELECT max(pk_user) + 1 FROM public.tb_user), 'new');
UPDATE public.tb_post SET title = title || '+' WHERE pk_post = 1;
INSERT INTO public.tb_comment (pk_comment, fk_post, body)
    VALUES ((SELECT max(pk_comment) + 1 FROM public.tb_comment), 2, 'c+');
DELETE FROM public.tb_comment WHERE pk_comment = (SELECT min(pk_comment) FROM public.tb_comment);
INSERT INTO public.tb_order (pk_order, fk_user, total)
    VALUES ((SELECT max(pk_order) + 1 FROM public.tb_order), 1, 1);
UPDATE app.tb_note SET body = body || '+';
-- The DISTINCT ON fixtures exist from 0.1.0-beta.22 on.
SELECT pg_catalog.to_regclass('public.tv_shipment') IS NOT NULL AS distinct_on_fixtures \gset
\if :distinct_on_fixtures
UPDATE public.tb_contract SET status = status || '+' WHERE id_contract = 100;
INSERT INTO public.tb_contract (pk_contract, id_contract, version_no, status)
    VALUES ((SELECT max(pk_contract) + 1 FROM public.tb_contract), 200,
            (SELECT max(version_no) + 1 FROM public.tb_contract WHERE id_contract = 200), 'v');
UPDATE public.tb_order SET total = total + 1 WHERE pk_order = 2;
UPDATE public.tb_shipment SET fk_order = 1 WHERE code = 's2';
\endif

DO $$
DECLARE
    tv TEXT;
    view pg_catalog.regclass;
    diverging BIGINT;
BEGIN
    FOREACH tv IN ARRAY ARRAY['public.user', 'public.post', 'public.comment', 'public.user_orders',
                              'public.contract', 'public.shipment', 'app.note'] LOOP
        CONTINUE WHEN pg_catalog.to_regclass(pg_catalog.replace(tv, '.', '.tv_')) IS NULL;
        -- The backing view by OID: <schema>.v_<entity> before 0.1.0-beta.25, in
        -- tviews after; the catalog is in the extension's schema (public in 0.1.0).
        EXECUTE pg_catalog.format(
            'SELECT view_oid::pg_catalog.oid::pg_catalog.regclass FROM %I.pg_tview_meta
             WHERE entity = %L',
            (SELECT n.nspname FROM pg_catalog.pg_extension e
             JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'pg_tviews'),
            pg_catalog.split_part(tv, '.', 2))
            INTO view;
        EXECUTE pg_catalog.format(
            'SELECT count(*) FROM ((SELECT pk_%2$s, data FROM %1$I.tv_%2$s
                                    EXCEPT SELECT pk_%2$s, data FROM %3$s)
                         UNION ALL (SELECT pk_%2$s, data FROM %3$s
                                    EXCEPT SELECT pk_%2$s, data FROM %1$I.tv_%2$s)) d',
            pg_catalog.split_part(tv, '.', 1), pg_catalog.split_part(tv, '.', 2),
            view)
            INTO diverging;
        IF diverging <> 0 THEN
            RAISE EXCEPTION 'upgrade check: % diverges from its view (% rows)', tv, diverging;
        END IF;
    END LOOP;
END $$;

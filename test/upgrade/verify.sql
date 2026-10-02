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

DO $$
DECLARE
    tv TEXT;
    diverging BIGINT;
BEGIN
    FOREACH tv IN ARRAY ARRAY['public.user', 'public.post', 'public.comment', 'public.user_orders',
                              'app.note'] LOOP
        EXECUTE pg_catalog.format(
            'SELECT count(*) FROM ((SELECT pk_%2$s, data FROM %1$I.tv_%2$s
                                    EXCEPT SELECT pk_%2$s, data FROM %1$I.v_%2$s)
                         UNION ALL (SELECT pk_%2$s, data FROM %1$I.v_%2$s
                                    EXCEPT SELECT pk_%2$s, data FROM %1$I.tv_%2$s)) d',
            pg_catalog.split_part(tv, '.', 1), pg_catalog.split_part(tv, '.', 2))
            INTO diverging;
        IF diverging <> 0 THEN
            RAISE EXCEPTION 'upgrade check: % diverges from its view (% rows)', tv, diverging;
        END IF;
    END LOOP;
END $$;

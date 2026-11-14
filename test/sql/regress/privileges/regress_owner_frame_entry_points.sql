-- A function that changes a TVIEW refuses to run inside pg_tviews' work as a
-- TVIEW's owner (E1-09).
--
-- Each function that changes a TVIEW (its definition, triggers, registration,
-- rows or indexes) is refused with 55000 inside an owner frame, and still runs
-- when called directly.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_owner_frame_entry_points.sql
-- expect-output: entry points refused inside an owner frame: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_ofe');

CREATE FUNCTION public.must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;

-- The calls to make from inside the refresh, and what each raised ('00000': ran).
CREATE TABLE public.calls (n int PRIMARY KEY, call text NOT NULL, raised text);
GRANT ALL ON public.calls TO PUBLIC;
CREATE FUNCTION public.make_calls() RETURNS int LANGUAGE plpgsql VOLATILE AS $$
DECLARE c record;
BEGIN
    FOR c IN SELECT n, call FROM public.calls WHERE raised IS NULL ORDER BY n LOOP
        BEGIN
            EXECUTE c.call;
            UPDATE public.calls SET raised = '00000' WHERE n = c.n;
        EXCEPTION WHEN OTHERS THEN
            UPDATE public.calls SET raised = SQLSTATE WHERE n = c.n;
        END;
    END LOOP;
    RETURN 0;
END $$;
-- Declared immutable, as a definition may only call such: run by every refresh.
CREATE FUNCTION public.hop(k int) RETURNS int LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN RETURN k + public.make_calls(); END $$;

SET ROLE regress_ofe_owner_a;
CREATE TABLE public.tb_a (pk_a int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          label text);
CREATE TABLE public.tb_b (pk_b int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          label text);
INSERT INTO public.tb_a (pk_a, label) VALUES (1, 'a');
INSERT INTO public.tb_b (pk_b, label) VALUES (1, 'b');
GRANT SELECT, UPDATE ON public.tb_a TO regress_ofe_writer;
SELECT tviews.pg_tviews_create('public.tv_a', $q$
    SELECT a.pk_a, a.id, jsonb_build_object('label', a.label, 'k', public.hop(length(a.label))) AS data
    FROM public.tb_a a $q$) IS NOT NULL AS created;
SELECT tviews.pg_tviews_create('public.tv_b', $q$
    SELECT b.pk_b, b.id, jsonb_build_object('label', b.label) AS data FROM public.tb_b b $q$)
    IS NOT NULL AS created;
RESET ROLE;
-- The owner is an operator too: the maintenance functions are its.
GRANT EXECUTE ON FUNCTION tviews.pg_tviews_refresh_all(), tviews.pg_tviews_rebuild_all(boolean),
    tviews.pg_tviews_reregister_all(boolean),
    tviews.pg_tviews_ensure_propagation_indexes_all(boolean)
    TO regress_ofe_owner_a;

INSERT INTO public.calls (n, call) VALUES
    (1, $c$SELECT tviews.pg_tviews_reregister('public.tv_b')$c$),
    (2, $c$SELECT * FROM tviews.pg_tviews_reregister_all(strict => true)$c$),
    (3, $c$SELECT * FROM tviews.pg_tviews_rebuild_all(true)$c$),
    (4, $c$SELECT * FROM tviews.pg_tviews_refresh_time_dependent()$c$),
    (5, $c$SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes('public.tv_b')$c$),
    (6, $c$SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes_all(false)$c$),
    (7, $c$SELECT tviews.pg_tviews_create_or_replace('public.tv_b', 'SELECT b.pk_b, b.id, jsonb_build_object(''label'', b.label, ''x'', 1) AS data FROM public.tb_b b')$c$),
    (8, $c$SELECT tviews.pg_tviews_refresh('public.tv_b')$c$),
    (9, $c$SELECT tviews.pg_tviews_refresh_all()$c$),
    (10, $c$SELECT * FROM tviews.pg_tviews_resync('public.tb_b', ARRAY[1::bigint])$c$),
    (11, $c$SELECT tviews.pg_tviews_create('public.tv_c', 'SELECT b.pk_b AS pk_c, b.id, b.data FROM public.tv_b b')$c$),
    (12, $c$SELECT tviews.pg_tviews_drop('public.tv_b')$c$);

-- The writer's write refreshes tv_a as its owner: hop() makes every call there.
SET ROLE regress_ofe_writer;
UPDATE public.tb_a SET label = 'a2';
RESET ROLE;

DO $$
DECLARE c record;
BEGIN
    FOR c IN SELECT n, call, raised FROM public.calls ORDER BY n LOOP
        PERFORM must(c.raised = '55000',
                     format('inside a refresh, %s was not refused (%s)', c.call,
                            coalesce(c.raised, 'not called')));
    END LOOP;
END $$;
SELECT must((SELECT data->>'label' FROM public.tv_a) = 'a2', 'the write was not refreshed');
SELECT must(EXISTS (SELECT FROM tviews.pg_tview_meta WHERE entity = 'b'), 'tv_b is gone');

-- Called directly, as the owner, each runs.
DELETE FROM public.calls;
SET ROLE regress_ofe_owner_a;
SELECT tviews.pg_tviews_reregister('public.tv_b');
SELECT tviews.pg_tviews_refresh('public.tv_b');
SELECT count(*) >= 0 AS ran FROM tviews.pg_tviews_ensure_propagation_indexes('public.tv_b');
SELECT tviews.pg_tviews_drop('public.tv_b');
RESET ROLE;

SELECT 'entry points refused inside an owner frame: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_a, public.tb_b, public.calls;
SELECT regress_drop_roles('regress_ofe');

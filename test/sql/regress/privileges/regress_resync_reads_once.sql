-- pg_tviews_resync reads the resynced rows once, as the caller; each TVIEW
-- reading the table maps those rows as its owner, and none of them reads the
-- table again.
--
-- Each reader's mapping runs as that reader's owner, one after the other. A
-- relation looked up by name for a later reader could be another relation by
-- then: the work of an earlier reader runs its owner's code. The later reader
-- maps the rows the caller read; the relations its mapping joins are named by
-- OID in its own frame.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_resync_reads_once.sql
-- expect-output: resync: rows read once, as the caller

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_ro');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'resync: %', what; END IF; END $$;

-- owner_a's table, read by its own TVIEW (sorting first) and by owner_b's. Its
-- TVIEW's mapping calls owner_a's swap(): once armed, it moves the table aside
-- and leaves a view of the same name in its place.
SET ROLE regress_ro_owner_a;
CREATE TABLE public.tb_region (pk_region int PRIMARY KEY, name text);
CREATE TABLE public.tb_area (pk_area int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_region int);
CREATE TABLE public.armed (on_ boolean);
INSERT INTO public.tb_region VALUES (1, 'r1');
INSERT INTO public.tb_area VALUES (1, DEFAULT, 1);
CREATE FUNCTION public.swap_now() RETURNS int LANGUAGE plpgsql VOLATILE AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM public.armed) THEN
        DELETE FROM public.armed;
        ALTER TABLE public.tb_region RENAME TO tb_region_aside;
        CREATE VIEW public.tb_region AS
            SELECT pk_region, name FROM public.tb_region_aside WHERE public.who('by name') = 1;
        GRANT SELECT ON public.tb_region TO PUBLIC;
    END IF;
    RETURN 0;
END $$;
CREATE FUNCTION public.swap(tag text) RETURNS int LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN RETURN public.swap_now(); END $$;
SELECT tviews.pg_tviews_create('public.tv_aaa', $q$
    SELECT a.pk_area AS pk_aaa, a.id, jsonb_build_object('id', a.id, 'region', r.name) AS data
    FROM public.tb_area a
    JOIN public.tb_region r ON r.pk_region = a.fk_region OPERATOR(pg_catalog.+) public.swap('x') $q$);
GRANT SELECT, TRIGGER ON public.tb_region TO regress_ro_owner_b;
RESET ROLE;

SET ROLE regress_ro_owner_b;
CREATE TABLE public.tb_shop (pk_shop int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_region int);
INSERT INTO public.tb_shop VALUES (1, DEFAULT, 1);
SELECT tviews.pg_tviews_create('public.tv_shop', $q$
    SELECT s.pk_shop, s.id, jsonb_build_object('id', s.id, 'region', r.name) AS data
    FROM public.tb_shop s JOIN public.tb_region r ON r.pk_region = s.fk_region $q$);
RESET ROLE;

-- The region's name changes while the TVIEWs miss it.
SET session_replication_role = replica;
UPDATE public.tb_region SET name = 'r1b';
SET session_replication_role = DEFAULT;
TRUNCATE public.who_log;

SET ROLE regress_ro_owner_a;
INSERT INTO public.armed VALUES (true);
SELECT * FROM tviews.pg_tviews_resync('public.tb_region', ARRAY[1]) ORDER BY 1;
RESET ROLE;

SELECT must(NOT EXISTS (SELECT 1 FROM public.who_log WHERE step = 'by name'),
            'a later reader read the table by name: ' ||
            coalesce((SELECT string_agg(DISTINCT cu, ', ') FROM public.who_log), ''));
SELECT must((SELECT data->>'region' FROM public.tv_shop WHERE pk_shop = 1) = 'r1b',
            'the later reader mapped the rows read');
SELECT 'resync: rows read once, as the caller' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP VIEW IF EXISTS public.tb_region;
DROP TABLE IF EXISTS public.tb_region, public.tb_region_aside, public.tb_area, public.tb_shop,
                     public.armed;
SELECT regress_drop_roles('regress_ro');

-- What a TVIEW holds stands for a read its owner may make: after the create too.
--
-- A refresh reads as the TVIEW's owner. A write the trigger can copy straight into
-- the TVIEW (a direct patch) stands for that read, so it is taken only where the
-- read would return the same: the owner may read every relation the view reads,
-- and no row security applies to the owner on them. Otherwise the row is
-- recomputed, which reads as the owner: a privilege the owner lost fails the write,
-- as an INSERT does, and a policy hides the row as it hides it from the read.
-- Re-registering (every update runs it) refuses a TVIEW whose owner may no longer
-- read what its view reads, and the health check names it.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_owner_reads.sql
-- expect-output: owner reads: all held

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_or');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'owner reads FAIL: %', what; END IF; END $$;

-- owner_b owns the tables and writes them; owner_a's TVIEWs read them.
SET ROLE regress_or_owner_b;
CREATE TABLE public.tb_src (pk_src int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL);
CREATE TABLE public.tb_pol (pk_pol int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL);
INSERT INTO public.tb_src (pk_src, title) VALUES (1, 's1');
INSERT INTO public.tb_pol (pk_pol, title) VALUES (1, 'p1'), (2, 'p2');
GRANT SELECT, TRIGGER ON public.tb_src, public.tb_pol TO regress_or_owner_a;
-- owner_a reads no row titled 'secret' of tb_pol; owner_b, its owner, is not bound.
ALTER TABLE public.tb_pol ENABLE ROW LEVEL SECURITY;
CREATE POLICY not_secret ON public.tb_pol FOR SELECT TO regress_or_owner_a
    USING (title <> 'secret');
RESET ROLE;

SET ROLE regress_or_owner_a;
SELECT tviews.pg_tviews_create('public.tv_src', $q$
    SELECT s.pk_src, s.id, jsonb_build_object('id', s.id, 'title', s.title) AS data
    FROM public.tb_src s $q$) IS NOT NULL AS created;
SELECT tviews.pg_tviews_create('public.tv_pol', $q$
    SELECT p.pk_pol, p.id, jsonb_build_object('id', p.id, 'title', p.title) AS data
    FROM public.tb_pol p $q$) IS NOT NULL AS created;
RESET ROLE;

-- 1. A copy the owner may read is patched in place.
SET ROLE regress_or_owner_b;
UPDATE public.tb_src SET title = 's1b' WHERE pk_src = 1;
UPDATE public.tb_pol SET title = 'p2b' WHERE pk_pol = 2;
RESET ROLE;
SELECT must((SELECT data->>'title' FROM public.tv_src WHERE pk_src = 1) = 's1b',
            'tv_src follows an update its owner may read');
SELECT must((SELECT data->>'title' FROM public.tv_pol WHERE pk_pol = 2) = 'p2b',
            'tv_pol follows an update its owner may read');

-- 2. An update moving a row out of the owner's policy leaves the TVIEW as the
--    owner's read does: without the row.
SET ROLE regress_or_owner_b;
UPDATE public.tb_pol SET title = 'secret' WHERE pk_pol = 1;
RESET ROLE;
SELECT must(NOT EXISTS (SELECT 1 FROM public.tv_pol WHERE pk_pol = 1),
            'a row the owner''s policy hides stayed in tv_pol: '
            || (SELECT data::text FROM public.tv_pol WHERE pk_pol = 1));
SELECT must((SELECT data->>'title' FROM public.tv_pol WHERE pk_pol = 2) = 'p2b',
            'tv_pol keeps the row its owner reads');

-- 3. The owner loses SELECT on tb_src: a write it no longer may read fails, as an
--    INSERT does, and the TVIEW keeps what its owner last read.
REVOKE SELECT ON public.tb_src FROM regress_or_owner_a;
DO $$
DECLARE
    failed boolean := false;
BEGIN
    BEGIN
        SET LOCAL ROLE regress_or_owner_b;
        UPDATE public.tb_src SET title = 's1c' WHERE pk_src = 1;
    EXCEPTION WHEN insufficient_privilege THEN
        failed := true;
    END;
    PERFORM must(failed, 'an update copied into tv_src for an owner without SELECT on tb_src');
END $$;
SELECT must((SELECT data->>'title' FROM public.tv_src WHERE pk_src = 1) = 's1b',
            'tv_src changed without its owner''s read');

-- 4. The health check names the TVIEW, its owner and the privilege.
SELECT must(status = 'ERROR' AND message LIKE '%tv_src%'
            AND message LIKE '%regress_or_owner_a lacks SELECT on public.tb_src%',
            format('owner_reads: %s %s', status, message))
FROM tviews.pg_tviews_check_health() WHERE component = 'owner_reads';
SELECT must(count(*) = 1, 'no owner_reads component in the health check')
FROM tviews.pg_tviews_check_health() WHERE component = 'owner_reads';

-- 5. Re-registering refuses it, naming the TVIEW, its owner and the privilege;
--    so does the strict re-registration every update runs.
DO $$
DECLARE
    state text;
    message text;
BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_reregister('src');
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, message = MESSAGE_TEXT;
    END;
    PERFORM must(state = '42501' AND message LIKE '%tv_src (regress_or_owner_a lacks SELECT on public.tb_src)%',
                 format('pg_tviews_reregister: %s %s', state, message));
    state := NULL;
    BEGIN
        PERFORM tviews.pg_tviews_reregister_all(strict => true);
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, message = MESSAGE_TEXT;
    END;
    PERFORM must(state = '42501' AND message LIKE '%tv_src (regress_or_owner_a lacks SELECT on public.tb_src)%',
                 format('pg_tviews_reregister_all(strict): %s %s', state, message));
END $$;

-- 6. Granted again, everything holds.
GRANT SELECT ON public.tb_src TO regress_or_owner_a;
SELECT must(status = 'OK', format('owner_reads after the grant: %s %s', status, message))
FROM tviews.pg_tviews_check_health() WHERE component = 'owner_reads';
SELECT count(*) AS reregistered FROM tviews.pg_tviews_reregister_all(strict => true);

SELECT 'owner reads: all held' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_src, public.tb_pol;
DROP FUNCTION must(boolean, text);
SELECT regress_drop_roles('regress_or');

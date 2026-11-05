-- pg_tviews_resync reads the resynced rows as the caller: it needs SELECT on
-- the table as well as UPDATE, and refuses a table whose row security applies to
-- the caller, whose read would miss the rows its policies hide.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_resync_caller_rights.sql
-- expect-output: resync: the caller's rights

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_rc');

CREATE FUNCTION refused(sql text, want text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE sql;
    RAISE EXCEPTION 'resync: not refused: %', sql;
EXCEPTION WHEN insufficient_privilege THEN
    IF SQLERRM NOT LIKE want THEN
        RAISE EXCEPTION 'resync: refused with %', SQLERRM;
    END IF;
    RETURN SQLSTATE;
END $$;
GRANT EXECUTE ON FUNCTION refused(text, text) TO PUBLIC;

SET ROLE regress_rc_owner_a;
CREATE TABLE public.tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             tenant text, name text);
INSERT INTO public.tb_user (pk_user, tenant, name) VALUES (1, 'a', 'u1'), (2, 'b', 'u2');
SELECT tviews.pg_tviews_create('public.tv_user', $q$
    SELECT u.pk_user, u.id, jsonb_build_object('id', u.id, 'name', u.name) AS data
    FROM public.tb_user u $q$);
GRANT UPDATE ON public.tb_user TO regress_rc_writer;
GRANT SELECT, UPDATE ON public.tb_user TO regress_rc_reader;
ALTER TABLE public.tb_user ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_b ON public.tb_user USING (tenant = 'b');
RESET ROLE;

-- UPDATE without SELECT.
SET ROLE regress_rc_writer;
SELECT refused($$SELECT * FROM tviews.pg_tviews_resync('public.tb_user', ARRAY[1])$$,
               '%SELECT on it is required%');
RESET ROLE;
-- SELECT and UPDATE, under row security.
SET ROLE regress_rc_reader;
SELECT refused($$SELECT * FROM tviews.pg_tviews_resync('public.tb_user', ARRAY[1])$$,
               '%row security active for you%');
RESET ROLE;
-- The owner, whom the policies do not bind.
SET session_replication_role = replica;
UPDATE public.tb_user SET name = 'u1b' WHERE pk_user = 1;
SET session_replication_role = DEFAULT;
SET ROLE regress_rc_owner_a;
SELECT * FROM tviews.pg_tviews_resync('public.tb_user', ARRAY[1]);
RESET ROLE;
DO $$ BEGIN
    IF (SELECT data->>'name' FROM public.tv_user WHERE pk_user = 1) <> 'u1b' THEN
        RAISE EXCEPTION 'resync: the owner''s resync did not refresh the row';
    END IF;
END $$;

-- A TVIEW the caller may not read is listed without its count.
CREATE TABLE public.tb_open (pk_open int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             name text);
INSERT INTO public.tb_open (pk_open, name) VALUES (1, 'o1');
SELECT tviews.pg_tviews_create('public.tv_open', $q$
    SELECT o.pk_open, o.id, jsonb_build_object('name', o.name) AS data FROM public.tb_open o $q$);
GRANT SELECT, UPDATE ON public.tb_open TO regress_rc_writer;
SET session_replication_role = replica;
UPDATE public.tb_open SET name = 'o1b' WHERE pk_open = 1;
SET session_replication_role = DEFAULT;
SET ROLE regress_rc_writer;
DO $$ BEGIN
    IF (SELECT rows_changed FROM tviews.pg_tviews_resync('public.tb_open', ARRAY[1])
        WHERE tview = 'public.tv_open') IS NOT NULL THEN
        RAISE EXCEPTION 'resync: a count was returned for a TVIEW the caller may not read';
    END IF;
END $$;
RESET ROLE;
DO $$ BEGIN
    IF (SELECT data->>'name' FROM public.tv_open WHERE pk_open = 1) <> 'o1b' THEN
        RAISE EXCEPTION 'resync: the row was not refreshed';
    END IF;
END $$;
GRANT SELECT ON public.tv_open TO regress_rc_writer;
SET session_replication_role = replica;
UPDATE public.tb_open SET name = 'o1c' WHERE pk_open = 1;
SET session_replication_role = DEFAULT;
SET ROLE regress_rc_writer;
DO $$ BEGIN
    IF (SELECT rows_changed FROM tviews.pg_tviews_resync('public.tb_open', ARRAY[1])
        WHERE tview = 'public.tv_open') IS DISTINCT FROM 1 THEN
        RAISE EXCEPTION 'resync: no count for a TVIEW the caller reads';
    END IF;
END $$;
RESET ROLE;

SELECT 'resync: the caller''s rights' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_user, public.tb_open;
SELECT regress_drop_roles('regress_rc');

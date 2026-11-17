-- Only the statements pg_tviews issues itself pass its hook unhandled; any other
-- statement is handled (E1-08).
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_nested_ddl_followed.sql
-- expect-output: DDL nested in pg_tviews' work is followed: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_ndf');

CREATE FUNCTION public.must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;

-- What the TVIEWs' own code does: grant a backing view to the reader.
CREATE TABLE public.grant_target (view regclass);
GRANT SELECT ON public.grant_target TO PUBLIC;
CREATE FUNCTION public.grant_backing_view() RETURNS int LANGUAGE plpgsql VOLATILE AS $$
DECLARE target regclass;
BEGIN
    SELECT view INTO target FROM public.grant_target;
    IF target IS NOT NULL THEN
        EXECUTE format('GRANT SELECT ON %s TO regress_ndf_reader', target);
    END IF;
    RETURN 1;
END $$;
-- Declared immutable, as a definition may only call such: run by the fill.
CREATE FUNCTION public.grant_backing_view_fill(n int) RETURNS int LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN RETURN n + public.grant_backing_view(); END $$;
CREATE FUNCTION public.grant_backing_view_trigger() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM public.grant_backing_view();
    RETURN NULL;
END $$;
-- Whether the reader was left a grant on `view`; the grant is taken back after.
CREATE FUNCTION granted(tv text) RETURNS boolean LANGUAGE plpgsql AS $$
DECLARE
    v regclass := (SELECT view_oid FROM tviews.pg_tview_meta WHERE 'tv_' || entity = tv)::regclass;
    held boolean := has_table_privilege('regress_ndf_reader', v, 'SELECT');
BEGIN
    EXECUTE format('REVOKE ALL ON %s FROM regress_ndf_reader', v);
    RETURN held;
END $$;
CREATE FUNCTION aim(tv text) RETURNS void LANGUAGE sql AS $$
    DELETE FROM public.grant_target;
    INSERT INTO public.grant_target
    SELECT view_oid::regclass FROM tviews.pg_tview_meta WHERE 'tv_' || entity = tv $$;

SET ROLE regress_ndf_owner_a;
CREATE TABLE public.tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text);
INSERT INTO public.tb_doc (pk_doc, title) VALUES (1, 'd1');
CREATE MATERIALIZED VIEW public.mv_doc AS SELECT pk_doc, id, title FROM public.tb_doc;
SELECT tviews.pg_tviews_create('public.tv_doc', $q$
    SELECT d.pk_doc, d.id, jsonb_build_object('title', d.title) AS data FROM public.tb_doc d $q$)
    IS NOT NULL AS created;
SELECT tviews.pg_tviews_create('public.tv_mdoc', $q$
    SELECT m.pk_doc AS pk_mdoc, m.id, jsonb_build_object('title', m.title) AS data
    FROM public.mv_doc m $q$, '{"uncascaded_policy": "full_refresh"}') IS NOT NULL AS created;
CREATE TRIGGER grant_on_write AFTER INSERT OR UPDATE ON public.tv_doc
    FOR EACH STATEMENT EXECUTE FUNCTION public.grant_backing_view_trigger();
CREATE TRIGGER grant_on_write AFTER INSERT OR UPDATE ON public.tv_mdoc
    FOR EACH STATEMENT EXECUTE FUNCTION public.grant_backing_view_trigger();
RESET ROLE;

-- The reference: a write's refresh, at the end of the writing statement.
SELECT aim('tv_doc');
SET ROLE regress_ndf_owner_a;
UPDATE public.tb_doc SET title = 'd1a';
RESET ROLE;
SELECT must(NOT granted('tv_doc'), 'a grant made by a write''s refresh was not followed');

-- The catch-up an explicit COMMIT runs for writes made while refresh was suspended.
SELECT aim('tv_doc');
BEGIN;
SELECT tviews.pg_tviews_suspend_triggers();
UPDATE public.tb_doc SET title = 'd1b';
COMMIT;
SELECT must(NOT granted('tv_doc'), 'a grant made by the catch-up at COMMIT was not followed');

-- The refresh that follows REFRESH MATERIALIZED VIEW.
SELECT aim('tv_mdoc');
SET ROLE regress_ndf_owner_a;
REFRESH MATERIALIZED VIEW public.mv_doc;
RESET ROLE;
SELECT must(NOT granted('tv_mdoc'), 'a grant made by the refresh after REFRESH MATERIALIZED VIEW was not followed');

-- The fill of CREATE TABLE tv_* AS, whose definition calls the function.
SELECT aim('tv_doc');
SET ROLE regress_ndf_owner_a;
CREATE TABLE public.tv_note AS
    SELECT d.pk_doc AS pk_note, d.id,
           jsonb_build_object('title', d.title, 'n', public.grant_backing_view_fill(d.pk_doc)) AS data
    FROM public.tb_doc d;
RESET ROLE;
SELECT must(NOT granted('tv_doc'), 'a grant made by the fill of CREATE TABLE AS was not followed');

-- Armed for one command tag on a table or view, the event trigger runs a
-- statement the hook refuses once and records its SQLSTATE ('00000': it ran).
CREATE TABLE public.armed (tag text, statement text);
CREATE TABLE public.outcome (sqlstate text);
GRANT ALL ON public.armed, public.outcome TO PUBLIC;
CREATE FUNCTION public.run_on_tag() RETURNS event_trigger LANGUAGE plpgsql AS $$
DECLARE todo text;
BEGIN
    IF NOT EXISTS (SELECT FROM pg_event_trigger_ddl_commands()
                   WHERE lower(object_type) IN ('table', 'view')) THEN
        RETURN;
    END IF;
    DELETE FROM public.armed WHERE tag = tg_tag RETURNING statement INTO todo;
    IF todo IS NOT NULL THEN
        BEGIN
            EXECUTE todo;
            INSERT INTO public.outcome VALUES ('00000');
        EXCEPTION WHEN OTHERS THEN
            INSERT INTO public.outcome VALUES (SQLSTATE);
        END;
    END IF;
END $$;
CREATE EVENT TRIGGER run_on_tag ON ddl_command_end EXECUTE FUNCTION public.run_on_tag();
CREATE FUNCTION refused(what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM must(NOT EXISTS (SELECT FROM public.armed), what || ' fired no armed event');
    PERFORM must((SELECT sqlstate FROM public.outcome) = '42809',
                 what || ': a column rename of a TVIEW table was not refused, but '
                 || coalesce((SELECT sqlstate FROM public.outcome), 'not run'));
    PERFORM must(EXISTS (SELECT FROM pg_attribute WHERE attrelid = 'public.tv_doc'::regclass
                                                    AND attname = 'data'),
                 what || ': tv_doc lost its data column');
    DELETE FROM public.outcome;
END $$;

-- An owner change: the backing view follows it (ALTER VIEW … OWNER TO).
INSERT INTO public.armed VALUES ('ALTER VIEW', 'ALTER TABLE public.tv_doc RENAME COLUMN data TO renamed');
ALTER TABLE public.tv_doc OWNER TO regress_ndf_owner_b;
SELECT refused('the backing view following an owner change');
ALTER TABLE public.tv_doc OWNER TO regress_ndf_owner_a;

-- A rebuild: the table's saved grants are restored on the rebuilt one.
GRANT SELECT ON public.tv_doc TO regress_ndf_writer;
INSERT INTO public.armed VALUES ('GRANT', 'ALTER TABLE public.tv_doc RENAME COLUMN data TO renamed');
SET ROLE regress_ndf_owner_a;
DROP TRIGGER grant_on_write ON public.tv_doc;
SELECT tviews.pg_tviews_create_or_replace('public.tv_doc', $q$
    SELECT d.pk_doc, d.id, d.title, jsonb_build_object('title', d.title) AS data
    FROM public.tb_doc d $q$);
RESET ROLE;
SELECT refused('a rebuild restoring grants');

DROP EVENT TRIGGER run_on_tag;

SELECT 'DDL nested in pg_tviews'' work is followed: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP MATERIALIZED VIEW public.mv_doc;
DROP TABLE public.tb_doc, public.grant_target, public.armed, public.outcome;
SELECT regress_drop_roles('regress_ndf');

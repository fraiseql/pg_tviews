-- pg_tviews_check_health() plans a TVIEW's mapping queries as the TVIEW's owner
-- only for a caller that has the owner's privileges.
--
-- The seq_scans component plans each mapping query, and planning evaluates the
-- immutable functions it calls: the owner's code. Anyone may call the health
-- check, so a TVIEW whose owner's privileges the caller lacks is counted, not
-- planned.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_health_as_owner.sql
-- expect-output: health: planned for the owner's members only

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_hc');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'health: %', what; END IF; END $$;

-- owner_b owns a table owner_a's TVIEW joins.
SET ROLE regress_hc_owner_b;
CREATE TABLE public.tb_tag (pk_tag int PRIMARY KEY, label text);
INSERT INTO public.tb_tag VALUES (1, 't1');
GRANT SELECT, TRIGGER ON public.tb_tag TO regress_hc_owner_a;
RESET ROLE;

SET ROLE regress_hc_owner_a;
CREATE TABLE public.tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            fk_tag int, title text);
INSERT INTO public.tb_doc VALUES (1, DEFAULT, 1, 'd1');
SELECT tviews.pg_tviews_create('public.tv_doc', $q$
    SELECT d.pk_doc, d.id, jsonb_build_object('id', d.id, 'tag', t.label) AS data
    FROM public.tb_doc d JOIN public.tb_tag t
      ON t.pk_tag = d.fk_tag OPERATOR(pg_catalog.+) public.folded('health') - 1 $q$);
RESET ROLE;
TRUNCATE public.who_log;

-- Neither the table's owner nor a stranger has the TVIEW owner's privileges.
SET ROLE regress_hc_owner_b;
SELECT status, message FROM tviews.pg_tviews_check_health() WHERE component = 'seq_scans';
RESET ROLE;
SET ROLE regress_hc_stranger;
SELECT status FROM tviews.pg_tviews_check_health() WHERE component = 'seq_scans';
RESET ROLE;
SELECT must(NOT EXISTS (SELECT 1 FROM public.who_log), 'planned for a caller without the owner''s privileges');
SET ROLE regress_hc_stranger;
SELECT must(message LIKE '%1 TVIEW of other owners not checked%', 'the skipped TVIEW is counted')
FROM tviews.pg_tviews_check_health() WHERE component = 'seq_scans';
RESET ROLE;

-- The owner, and a member of its role, get it planned, as the owner.
GRANT regress_hc_owner_a TO regress_hc_reader;
SET ROLE regress_hc_reader;
SELECT status FROM tviews.pg_tviews_check_health() WHERE component = 'seq_scans';
RESET ROLE;
SELECT must((SELECT array_agg(DISTINCT cu) FROM public.who_log) = ARRAY['regress_hc_owner_a']::name[],
            'planned as the owner for a member of its role');

SELECT 'health: planned for the owner''s members only' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_tag, public.tb_doc;
SELECT regress_drop_roles('regress_hc');

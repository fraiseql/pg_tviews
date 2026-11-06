-- Re-deriving a TVIEW runs as its owner, whoever triggers the re-derivation.
--
-- Re-deriving plans the TVIEW's mapping queries, and planning evaluates the
-- immutable functions they call with constant arguments: the owner's code. Like
-- every refresh, it runs as the TVIEW's owner: for pg_tviews_reregister(),
-- pg_tviews_reregister_all() (the update script calls it), create_or_replace
-- with only its options changed, and the column rename of a table it reads by
-- that table's owner.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_reregister_as_owner.sql
-- expect-output: re-derivation: all as the owner

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_rr');

-- owner_b owns a table owner_a's TVIEW reads, and grants what that needs.
SET ROLE regress_rr_owner_b;
CREATE TABLE public.tb_tag (pk_tag int PRIMARY KEY, label text);
INSERT INTO public.tb_tag VALUES (1, 't1');
GRANT SELECT, TRIGGER ON public.tb_tag TO regress_rr_owner_a;
RESET ROLE;

-- folded('derive') sits in the mapping query of tb_note, which registration
-- plans.
SET ROLE regress_rr_owner_a;
CREATE TABLE public.tb_doc (pk_doc int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            fk_tag int, title text);
CREATE TABLE public.tb_note (pk_note int PRIMARY KEY, fk_doc int, body text);
INSERT INTO public.tb_doc VALUES (1, DEFAULT, 1, 'd1');
INSERT INTO public.tb_note VALUES (1, 1, 'n1');
SELECT tviews.pg_tviews_create('public.tv_doc', $q$
    SELECT d.pk_doc, d.id, jsonb_build_object('id', d.id, 'title', d.title, 'tag', t.label,
        'notes', (SELECT jsonb_agg(n.body) FROM public.tb_note n
                  WHERE n.fk_doc = d.pk_doc OPERATOR(pg_catalog.+) public.folded('derive') - 1)) AS data
    FROM public.tb_doc d LEFT JOIN public.tb_tag t ON t.pk_tag = d.fk_tag $q$);
RESET ROLE;

SELECT count(*) > 0 AS planned_at_create FROM public.who_log WHERE cu = 'regress_rr_owner_a';
TRUNCATE public.who_log;

-- Each path, by the superuser.
SELECT tviews.pg_tviews_reregister('doc');
SELECT count(*) FROM tviews.pg_tviews_reregister_all();
-- What the update script runs.
UPDATE tviews.pg_tview_meta SET needs_reregister = true;
SELECT count(*) FROM tviews.pg_tviews_reregister_all(strict => true);
SELECT tviews.pg_tviews_create_or_replace('public.tv_doc', definition,
                                   options => '{"uncascaded_policy": "warn"}')
FROM tviews.pg_tview_meta WHERE entity = 'doc';
ALTER TABLE public.tb_note RENAME COLUMN body TO text_body;

-- The rename of a column of tb_tag by its owner, who is not the TVIEW's owner.
SET ROLE regress_rr_owner_b;
ALTER TABLE public.tb_tag RENAME COLUMN label TO tag_label;
RESET ROLE;

SELECT count(*) AS rederived FROM public.who_log;
DO $$
DECLARE
    others text;
BEGIN
    SELECT string_agg(DISTINCT cu, ', ') INTO others FROM public.who_log
    WHERE cu <> 'regress_rr_owner_a';
    IF others IS NOT NULL THEN
        RAISE EXCEPTION 'the TVIEW''s functions ran as %', others;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM public.who_log) THEN
        RAISE EXCEPTION 'no re-derivation planned the mapping query';
    END IF;
END $$;
SELECT 're-derivation: all as the owner' AS result;

-- The TVIEW still refreshes after the renames.
INSERT INTO public.tb_note VALUES (2, 1, 'n2');
SELECT data->'notes' AS notes FROM public.tv_doc;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_tag, public.tb_doc, public.tb_note;
SELECT regress_drop_roles('regress_rr');

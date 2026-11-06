-- pg_tviews_show_replication() answers any caller. It says whether a TVIEW is
-- empty only to a caller who may read it, without row security applying: the
-- answer comes from a read as the caller, never as the TVIEW's owner. Elsewhere
-- is_empty is NULL.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_show_replication_acl.sql
-- expect-output: show_replication: emptiness for readers only

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_sr');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'show_replication: %', what; END IF; END $$;
GRANT EXECUTE ON FUNCTION must(boolean, text) TO PUBLIC;

CREATE TABLE public.tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             name text);
CREATE TABLE public.tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             title text);
INSERT INTO public.tb_user (pk_user, name) VALUES (1, 'u1');
SELECT tviews.pg_tviews_create('public.tv_user', $q$
    SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) AS data FROM public.tb_user u $q$);
SELECT tviews.pg_tviews_create('public.tv_post', $q$
    SELECT p.pk_post, p.id, jsonb_build_object('title', p.title) AS data FROM public.tb_post p $q$);
GRANT SELECT ON public.tv_user TO regress_sr_reader;

-- A role with no privilege on either TVIEW.
SET ROLE regress_sr_stranger;
SELECT must(count(*) = 2 AND bool_and(is_empty IS NULL), 'the stranger''s rows')
FROM tviews.pg_tviews_show_replication();
RESET ROLE;
-- SELECT on tv_user only.
SET ROLE regress_sr_reader;
SELECT must((SELECT is_empty FROM tviews.pg_tviews_show_replication() WHERE entity = 'user') = false,
            'a readable TVIEW''s emptiness');
SELECT must((SELECT is_empty FROM tviews.pg_tviews_show_replication() WHERE entity = 'post') IS NULL,
            'an unreadable TVIEW''s emptiness');
RESET ROLE;
-- Row security on tv_user applies to the reader.
ALTER TABLE public.tv_user ENABLE ROW LEVEL SECURITY;
CREATE POLICY none ON public.tv_user USING (false);
SET ROLE regress_sr_reader;
SELECT must((SELECT is_empty FROM tviews.pg_tviews_show_replication() WHERE entity = 'user') IS NULL,
            'emptiness under row security');
RESET ROLE;
-- The owner.
SELECT must((SELECT array_agg(is_empty ORDER BY entity) FROM tviews.pg_tviews_show_replication())
            = ARRAY[true, false], 'the owner''s rows');

SELECT 'show_replication: emptiness for readers only' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_user, public.tb_post;
SELECT regress_drop_roles('regress_sr');

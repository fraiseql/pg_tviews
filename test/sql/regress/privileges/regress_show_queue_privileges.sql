-- pg_tviews_show_queue() lists the queued keys of the TVIEWs the caller may
-- read, as reading them would show: a TVIEW the caller may not SELECT, or whose
-- row security applies to the caller, is named by type in `omitted_types`, its
-- keys left out. `queue_size` counts every key, as tviews.stats does.
--
-- A mutation function's writes stay queued until its statement ends (ADR 0286),
-- so a function the caller runs sees what its writes queued.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_show_queue_privileges.sql
-- expect-output: show_queue: keys of readable TVIEWs only

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_sq');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'show_queue: %', what; END IF; END $$;

CREATE TABLE public.tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             name text);
CREATE TABLE public.tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             title text);
INSERT INTO public.tb_user (pk_user, name) VALUES (1, 'u1');
INSERT INTO public.tb_post (pk_post, title) VALUES (1, 'p1');
SELECT tviews.pg_tviews_create('public.tv_user', $q$
    SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) AS data FROM public.tb_user u $q$);
SELECT tviews.pg_tviews_create('public.tv_post', $q$
    SELECT p.pk_post, p.id, jsonb_build_object('title', p.title) AS data FROM public.tb_post p $q$);
GRANT SELECT, UPDATE ON public.tb_user, public.tb_post TO regress_sq_writer;
GRANT SELECT ON public.tv_user TO regress_sq_writer;

-- Writes, then what is queued, in one statement.
CREATE FUNCTION public.write_and_peek() RETURNS jsonb LANGUAGE plpgsql AS $$
BEGIN
    UPDATE public.tb_user SET name = name || '!';
    UPDATE public.tb_post SET title = title || '!';
    RETURN tviews.pg_tviews_show_queue();
END $$;
GRANT EXECUTE ON FUNCTION public.write_and_peek() TO PUBLIC;

-- No SELECT on tv_post.
SET ROLE regress_sq_writer;
SELECT public.write_and_peek() AS q \gset
RESET ROLE;
SELECT must(NOT jsonb_path_exists(:'q', '$.queued[*] ? (@.entity == "post")'),
            'keys of a TVIEW the caller may not read: ' || :'q');
SELECT must(jsonb_path_exists(:'q', '$.queued[*] ? (@.entity == "user")'),
            'keys of a readable TVIEW missing: ' || :'q');
SELECT must((:'q'::jsonb)->'omitted_types' = '["Post"]', 'omitted_types: ' || :'q');
SELECT must(((:'q'::jsonb)->>'queue_size')::int = 2, 'queue_size counts every key: ' || :'q');

-- Row security on tv_user applies to the writer.
ALTER TABLE public.tv_user ENABLE ROW LEVEL SECURITY;
CREATE POLICY own ON public.tv_user USING (pk_user = 0);
SET ROLE regress_sq_writer;
SELECT public.write_and_peek() AS q \gset
RESET ROLE;
SELECT must(NOT jsonb_path_exists(:'q', '$.queued[*]'), 'keys under row security: ' || :'q');
SELECT must((:'q'::jsonb)->'omitted_types' = '["Post", "User"]', 'omitted_types under RLS: ' || :'q');

-- The owner sees every key.
SELECT public.write_and_peek() AS q \gset
SELECT must(jsonb_array_length((:'q'::jsonb)->'queued') = 2 AND NOT (:'q'::jsonb) ? 'omitted_types',
            'the owner''s view: ' || :'q');

SELECT 'show_queue: keys of readable TVIEWs only' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_user, public.tb_post;
DROP FUNCTION public.write_and_peek();
SELECT regress_drop_roles('regress_sq');

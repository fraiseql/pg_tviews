-- A TVIEW reading another role's TVIEW needs the TRIGGER privilege on that
-- TVIEW's table, as it does on a base table it reads: the reader's refreshes
-- then run as part of the other TVIEW's writes. A TVIEW reading its own
-- owner's TVIEWs needs nothing more, and a replace keeps reading a table it
-- already read.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_embed_trigger_privilege.sql
-- expect-output: embed trigger privilege: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_et');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'embed trigger privilege: %', what; END IF; END $$;
-- The SQLSTATE and message of `sql`, or NULLs when it succeeds.
CREATE FUNCTION attempt(sql text, OUT state text, OUT message text) LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE sql;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, message = MESSAGE_TEXT;
END $$;
GRANT EXECUTE ON FUNCTION attempt(text) TO PUBLIC;

SET ROLE regress_et_owner_a;
CREATE TABLE public.tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             title text);
INSERT INTO public.tb_post (pk_post, title) VALUES (1, 'one');
SELECT tviews.pg_tviews_create('public.tv_post', $q$
    SELECT p.pk_post, p.id, jsonb_build_object('title', p.title) AS data FROM public.tb_post p $q$);
GRANT SELECT ON public.tv_post TO regress_et_owner_b;
RESET ROLE;

SET ROLE regress_et_owner_b;
CREATE TABLE public.tb_comment (pk_comment int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                                fk_post int, body text);
INSERT INTO public.tb_comment (pk_comment, fk_post, body) VALUES (1, 1, 'first');
CREATE TEMP TABLE tried AS
SELECT * FROM attempt($a$SELECT tviews.pg_tviews_create('public.tv_comment', $q$
    SELECT c.pk_comment, c.id, c.fk_post,
           jsonb_build_object('body', c.body, 'post', p.data) AS data
    FROM public.tb_comment c JOIN public.tv_post p ON p.pk_post = c.fk_post $q$)$a$);
SELECT state, message FROM tried;
SELECT must(state = '42501', format('reading another role''s TVIEW without TRIGGER: %s', state))
FROM tried;
SELECT must(message LIKE '%tv_post%TRIGGER%', format('the refusal names the table: %s', message))
FROM tried;
SELECT must(to_regclass('public.tv_comment') IS NULL, 'the refused TVIEW was kept');
RESET ROLE;

-- Granted, the same TVIEW is created.
GRANT TRIGGER ON public.tv_post TO regress_et_owner_b;
SET ROLE regress_et_owner_b;
SELECT tviews.pg_tviews_create('public.tv_comment', $q$
    SELECT c.pk_comment, c.id, c.fk_post,
           jsonb_build_object('body', c.body, 'post', p.data) AS data
    FROM public.tb_comment c JOIN public.tv_post p ON p.pk_post = c.fk_post $q$);
RESET ROLE;

-- Revoked, a replace still reads the table it read.
REVOKE TRIGGER ON public.tv_post FROM regress_et_owner_b;
SET ROLE regress_et_owner_b;
SELECT tviews.pg_tviews_create_or_replace('public.tv_comment', $q$
    SELECT c.pk_comment, c.id, c.fk_post,
           jsonb_build_object('body', upper(c.body), 'post', p.data) AS data
    FROM public.tb_comment c JOIN public.tv_post p ON p.pk_post = c.fk_post $q$);
SELECT must(data->>'body' = 'FIRST', 'the replace') FROM public.tv_comment;
RESET ROLE;

-- The same owner reads its own TVIEW without a grant.
SET ROLE regress_et_owner_a;
CREATE TABLE public.tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            fk_post int, label text);
INSERT INTO public.tb_tag (pk_tag, fk_post, label) VALUES (1, 1, 'news');
SELECT tviews.pg_tviews_create('public.tv_tag', $q$
    SELECT t.pk_tag, t.id, t.fk_post,
           jsonb_build_object('label', t.label, 'post', p.data) AS data
    FROM public.tb_tag t JOIN public.tv_post p ON p.pk_post = t.fk_post $q$);
UPDATE public.tb_post SET title = 'uno' WHERE pk_post = 1;
SELECT must(data->'post'->>'title' = 'uno', 'the same owner''s embed') FROM public.tv_tag;
RESET ROLE;

SELECT 'embed trigger privilege: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_tag, public.tb_comment, public.tb_post;
DROP FUNCTION must(boolean, text), attempt(text);
SELECT regress_drop_roles('regress_et');

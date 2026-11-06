-- A GraphQL type name names one TVIEW: the report and the change feed key
-- entries by it. Creating or replacing a TVIEW whose type name (its typename
-- option, else the PascalCase of its entity) another TVIEW holds is refused
-- with 42710, whoever owns the other one; a name freed by a drop can be taken.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_typename_unique.sql
-- expect-output: typename unique: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_tn');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'typename unique: %', what; END IF; END $$;
-- The SQLSTATE of `sql`, or NULL when it succeeds.
CREATE FUNCTION state_of(sql text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE s text;
BEGIN
    EXECUTE sql;
    RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS s = RETURNED_SQLSTATE;
    RETURN s;
END $$;
GRANT EXECUTE ON FUNCTION must(boolean, text), state_of(text) TO PUBLIC;
-- A definition of tv_<entity> over tb_note.
CREATE FUNCTION def(entity text) RETURNS text LANGUAGE sql AS $$
    SELECT format('SELECT n.pk_note AS pk_%s, n.id, jsonb_build_object(''body'', n.body) AS data '
                  'FROM public.tb_note n', entity) $$;
GRANT EXECUTE ON FUNCTION def(text) TO PUBLIC;
CREATE FUNCTION typename_of(entity text) RETURNS text LANGUAGE sql AS $$
    SELECT typename FROM tviews.registry WHERE entity = $1 $$;

CREATE TABLE public.tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             body text);
INSERT INTO public.tb_note (pk_note, body) VALUES (1, 'n1');
GRANT SELECT, TRIGGER ON public.tb_note TO PUBLIC;

SELECT tviews.pg_tviews_create('public.tv_post', def('post'));

-- A default name another TVIEW holds: `post_` is `Post` too.
SELECT must(state_of(format('SELECT tviews.pg_tviews_create(%L, %L)',
                            'public.tv_post_', def('post_'))) = '42710',
            'a default name another TVIEW holds');
-- The same entity under a name of its own.
SELECT tviews.pg_tviews_create('public.tv_post_', def('post_'), '{"typename": "PostDraft"}');
SELECT must(typename_of('post_') = 'PostDraft', 'the declared name was not stored');
-- A declared name another TVIEW holds.
SELECT must(state_of(format('SELECT tviews.pg_tviews_create(%L, %L, %L)',
                            'public.tv_article', def('article'), '{"typename": "Post"}')) = '42710',
            'a declared name another TVIEW holds');
-- A replace taking a name another TVIEW holds, by option and keeping the definition.
SELECT tviews.pg_tviews_create('public.tv_blog', def('blog'));
SELECT must(state_of(format('SELECT tviews.pg_tviews_create_or_replace(%L, %L, %L)',
                            'public.tv_blog', def('blog'), '{"typename": "PostDraft"}')) = '42710',
            'a replace taking a name another TVIEW holds');
SELECT must(typename_of('blog') = 'Blog', 'the refused replace changed the name');
-- A replace keeping its own name.
SELECT tviews.pg_tviews_create_or_replace('public.tv_blog', def('blog'), '{"typename": "Blog"}');

-- Another owner's TVIEW holding the name.
SET ROLE regress_tn_owner_b;
SELECT must(state_of(format('SELECT tviews.pg_tviews_create(%L, %L, %L)',
                            'public.tv_memo', def('memo'), '{"typename": "Post"}')) = '42710',
            'a name another owner''s TVIEW holds');
RESET ROLE;

-- A name a dropped TVIEW held.
SELECT tviews.pg_tviews_drop('public.tv_post');
SELECT tviews.pg_tviews_create('public.tv_article', def('article'), '{"typename": "Post"}');
SELECT must(typename_of('article') = 'Post', 'a freed name could not be taken');

SELECT 'typename unique: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_note;
DROP FUNCTION must(boolean, text), state_of(text), def(text), typename_of(text);
SELECT regress_drop_roles('regress_tn');

-- The tag of a value lock, which pg_locks shows to every role, is keyed with a
-- secret of the server: it is not a function of the value alone, so no role
-- can tell from it which value is locked. A superuser computes the tag of a
-- value with pg_tviews_value_lock_tag; no other role may call it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_value_lock_tags.sql
-- expect-output: value lock tags: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_vt');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'value lock tags: %', what; END IF; END $$;
-- 32-bit FNV-1a: the unkeyed hash of the bytes given.
CREATE FUNCTION fnv1a(b bytea) RETURNS bigint LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE h bigint := 2166136261;
BEGIN
    FOR i IN 0 .. length(b) - 1 LOOP
        h := ((h # get_byte(b, i)) * 16777619) % 4294967296;
    END LOOP;
    RETURN h;
END $$;
GRANT EXECUTE ON FUNCTION must(boolean, text), fnv1a(bytea) TO PUBLIC;

CREATE TABLE public.tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             name text);
CREATE TABLE public.tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_user int NOT NULL, title text);
INSERT INTO public.tb_user (pk_user, name) SELECT g, 'u' || g FROM generate_series(1, 20) g;
SELECT tviews.pg_tviews_create('public.tv_post', $q$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM public.tb_post p JOIN public.tb_user u ON u.pk_user = p.fk_user $q$);
GRANT INSERT ON public.tb_post TO regress_vt_writer;

BEGIN;
SET LOCAL ROLE regress_vt_writer;
-- A new post links user 7: the transaction holds value locks on it.
INSERT INTO public.tb_post (pk_post, fk_user, title) VALUES (1, 7, 'p');
CREATE TEMP TABLE held ON COMMIT DROP AS
SELECT objid::bigint AS tag FROM pg_locks
WHERE pid = pg_backend_pid() AND locktype = 'advisory' AND objsubid = 21622;
SELECT must(count(*) > 0, 'the write took no value lock') FROM held;
-- The unkeyed hash of every value 1..1000 of either join column, in the lock
-- form of an integer (its hash, as text), matches no held tag.
-- The attnums (1: tb_user.pk_user, 3: tb_post.fk_user) as little-endian int2.
SELECT must(NOT EXISTS (
    SELECT 1 FROM held h,
         (VALUES ('\x0100'::bytea), ('\x0300'::bytea)) a(attnum),
         generate_series(1, 1000) v
    WHERE h.tag = fnv1a(a.attnum || '\x00'::bytea || convert_to(hashint4(v)::text, 'UTF8'))),
    'a held tag is the unkeyed hash of a value');
ROLLBACK;

-- Only a superuser computes a tag.
SET ROLE regress_vt_writer;
\set ON_ERROR_STOP off
SELECT tviews.pg_tviews_value_lock_tag('public.tb_user', 'pk_user', '7');
\set ON_ERROR_STOP on
SELECT must(:'SQLSTATE' = '42501', format('a writer computed a tag: %s', :'SQLSTATE'));
RESET ROLE;

BEGIN;
SET LOCAL ROLE regress_vt_writer;
INSERT INTO public.tb_post (pk_post, fk_user, title) VALUES (1, 7, 'p');
RESET ROLE;
-- The superuser's tags of user 7, on the user's key and the post's link, are
-- among those held; those of user 8 are not.
SELECT must(EXISTS (
    SELECT 1 FROM pg_locks l
    WHERE l.pid = pg_backend_pid() AND l.locktype = 'advisory' AND l.objsubid = 21622
      AND l.objid::bigint IN (tviews.pg_tviews_value_lock_tag('public.tb_user', 'pk_user', '7'),
                              tviews.pg_tviews_value_lock_tag('public.tb_post', 'fk_user', '7'))),
    'no held tag is the superuser''s tag of the linked user');
SELECT must(NOT EXISTS (
    SELECT 1 FROM pg_locks l
    WHERE l.pid = pg_backend_pid() AND l.locktype = 'advisory' AND l.objsubid = 21622
      AND l.objid::bigint IN (tviews.pg_tviews_value_lock_tag('public.tb_user', 'pk_user', '8'),
                              tviews.pg_tviews_value_lock_tag('public.tb_post', 'fk_user', '8'))),
    'a held tag is the tag of a user not linked');
ROLLBACK;

-- A column the relation lacks is refused.
\set ON_ERROR_STOP off
SELECT tviews.pg_tviews_value_lock_tag('public.tb_user', 'nope', '7');
\set ON_ERROR_STOP on
SELECT must(:'SQLSTATE' = '22023', format('a missing column: %s', :'SQLSTATE'));

SELECT 'value lock tags: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_post, public.tb_user;
DROP FUNCTION must(boolean, text), fnv1a(bytea);
SELECT regress_drop_roles('regress_vt');

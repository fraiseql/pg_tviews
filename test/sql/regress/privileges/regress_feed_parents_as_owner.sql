-- The change feed names a written row's collections by their parents' public ids
-- (pg_tviews.publish_changes = rows), read from the parents' TVIEWs as their
-- owners, as every other refresh read is.
--
-- The read ran as the writer: a writer without SELECT on the parent TVIEW failed
-- with "permission denied for table tv_post", and a policy on the parent TVIEW
-- was evaluated as the writer, not as the owner.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_feed_parents_as_owner.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS app CASCADE;
DROP TABLE IF EXISTS public.ran_as;
DROP ROLE IF EXISTS regress_feed_writer;
DROP ROLE IF EXISTS regress_feed_owner;
CREATE ROLE regress_feed_owner;
CREATE ROLE regress_feed_writer;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
GRANT USAGE ON SCHEMA tviews TO regress_feed_owner;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'feed parents FAIL: %', what; END IF; END $$;

CREATE TABLE public.ran_as (who text);
GRANT INSERT ON public.ran_as TO PUBLIC;
CREATE SCHEMA app AUTHORIZATION regress_feed_owner;

SET ROLE regress_feed_owner;
CREATE TABLE app.tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          title text);
CREATE TABLE app.tb_comment (pk_comment int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_post int NOT NULL REFERENCES app.tb_post, body text);
INSERT INTO app.tb_post (pk_post, title) VALUES (1, 'one'), (2, 'two');
INSERT INTO app.tb_comment (pk_comment, fk_post, body) VALUES (1, 1, 'c');
SELECT tviews.pg_tviews_create('app.tv_post', $$
    SELECT p.pk_post, p.id, jsonb_build_object('title', p.title) AS data FROM app.tb_post p $$)
    IS NOT NULL;
SELECT tviews.pg_tviews_create('app.tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post, jsonb_build_object('body', c.body) AS data
    FROM app.tb_comment c $$) IS NOT NULL;
-- A policy calling the owner's function: it records who runs it.
CREATE FUNCTION app.seen() RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN INSERT INTO public.ran_as VALUES (current_user); RETURN true; END $$;
ALTER TABLE app.tv_post ENABLE ROW LEVEL SECURITY;
CREATE POLICY everyone ON app.tv_post USING (app.seen());
GRANT USAGE ON SCHEMA app TO regress_feed_writer;
GRANT SELECT, UPDATE ON app.tb_comment TO regress_feed_writer;
RESET ROLE;
SELECT must((SELECT count(*) FROM tviews.registry r, jsonb_each(r.references) e
             WHERE r.entity = 'comment') = 1,
            'tv_comment names no collection of tv_post: the test reads nothing');

-- 1. A writer without SELECT on tv_post.
SET pg_tviews.publish_changes = rows;
SET ROLE regress_feed_writer;
UPDATE app.tb_comment SET fk_post = 2 WHERE pk_comment = 1;
RESET ROLE;
SELECT must((SELECT fk_post FROM app.tv_comment WHERE pk_comment = 1) = 2,
            'tv_comment does not follow the move');

-- 2. A writer allowed to read tv_post: the owner's policy never runs as it.
GRANT SELECT ON app.tv_post TO regress_feed_writer;
SET ROLE regress_feed_writer;
UPDATE app.tb_comment SET fk_post = 1 WHERE pk_comment = 1;
RESET ROLE;
RESET pg_tviews.publish_changes;
SELECT must(NOT EXISTS (SELECT FROM public.ran_as WHERE who = 'regress_feed_writer'),
            'the parent TVIEW''s policy ran as the writer');

DROP SCHEMA app CASCADE;
DROP TABLE public.ran_as;
REVOKE USAGE ON SCHEMA tviews FROM regress_feed_owner;
DROP ROLE regress_feed_writer;
DROP ROLE regress_feed_owner;

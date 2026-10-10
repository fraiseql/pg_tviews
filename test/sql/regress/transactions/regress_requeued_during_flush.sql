-- A key queued again by a trigger the flush's own writes fire is refreshed again.
--
-- tv_post embeds tv_user, and a user trigger on tv_post's table writes back to
-- tb_user. A rename refreshes tv_user, then tv_post; tv_post's trigger then
-- changes the user row tv_user already refreshed. The flush refreshes it once
-- more instead of dropping the new work, which left tv_user stale with no error.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_requeued_during_flush.sql
-- expect-output: requeued_during_flush: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, posts_touched int NOT NULL DEFAULT 0);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'touched', posts_touched) AS data
    FROM tb_user
$$);
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user
$$);

-- Counts every refresh of a post into its author's row. Runs inside the flush,
-- whose search_path is pg_catalog, pg_temp: names are qualified.
CREATE FUNCTION public.touch_author() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE public.tb_user SET posts_touched = posts_touched + 1 WHERE pk_user = NEW.fk_user;
    RETURN NULL;
END $$;
CREATE TRIGGER touch_author AFTER UPDATE ON tv_post
    FOR EACH ROW WHEN (OLD.data IS DISTINCT FROM NEW.data) EXECUTE FUNCTION public.touch_author();

UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
SELECT assert_fresh('tv_user', 'pk_user', 'a rename whose cascade wrote the user row again');

\echo 'requeued_during_flush: PASS'

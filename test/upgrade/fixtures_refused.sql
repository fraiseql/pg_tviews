-- Catalogs the update refuses (test/upgrade/upgrade_check.sh), each in a database
-- of its own, created with the PREVIOUS release. psql variable `case`:
--
-- - typename_clash: tv_post and tv_post_ both have the GraphQL type name Post;
--   the update names both. A release that refuses the clash at create time cannot
--   hold one: the database then records why in upgrade_unreachable, and the check
--   reports the case instead of updating it.
--
-- A TVIEW the update cannot re-derive does not refuse it: fixtures_left_behind.sql.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
SET search_path TO "$user", public, tviews;

CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text);
INSERT INTO tb_post (pk_post, title) VALUES (1, 'p1');

SELECT :'case' = 'typename_clash' AS typename_clash \gset
\if :typename_clash
SELECT pg_tviews_create('tv_post', $$
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post $$);
DO $do$ BEGIN
    PERFORM pg_tviews_create('tv_post_', $$
        SELECT pk_post AS pk_post_, id, jsonb_build_object('title', title) AS data FROM tb_post $$);
EXCEPTION WHEN duplicate_object THEN
    IF SQLERRM NOT LIKE '%is already the type of tv_post''s rows%' THEN
        RAISE;
    END IF;
    CREATE TABLE public.upgrade_unreachable AS SELECT SQLERRM AS reason;
END $do$;
\endif

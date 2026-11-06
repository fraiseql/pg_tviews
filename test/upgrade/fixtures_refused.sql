-- Catalogs the update refuses (test/upgrade/upgrade_check.sh), each in a database
-- of its own, created with the PREVIOUS release. psql variable `case`:
--
-- - typename_clash: tv_post and tv_post_ both have the GraphQL type name Post;
--   the update names both.
-- - lost_privilege: upgrade_lost_owner's TVIEW reads a table it may no longer
--   read; the update names the TVIEW, its owner and the privilege.

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
SELECT pg_tviews_create('tv_post_', $$
    SELECT pk_post AS pk_post_, id, jsonb_build_object('title', title) AS data FROM tb_post $$);
\else
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'upgrade_lost_owner') THEN
        CREATE ROLE upgrade_lost_owner;
    END IF;
END $$;
GRANT CREATE ON SCHEMA public TO upgrade_lost_owner;
GRANT SELECT, TRIGGER ON tb_post TO upgrade_lost_owner;
SET ROLE upgrade_lost_owner;
SELECT pg_tviews_create('tv_lost', $$
    SELECT pk_post AS pk_lost, id, jsonb_build_object('title', title) AS data FROM public.tb_post $$);
RESET ROLE;
REVOKE SELECT ON tb_post FROM upgrade_lost_owner;
\endif

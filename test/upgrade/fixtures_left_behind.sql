-- A catalog the update completes on, leaving TVIEWs it cannot re-derive in
-- needs_reregister (test/upgrade/upgrade_check.sh), in a database of its own,
-- created with the PREVIOUS release. The update does not depend on what a
-- TVIEW's owner chose: it names each TVIEW it leaves, and the health check
-- lists them.
--
-- - tv_kept: re-derived.
-- - tv_forced: its stored definition is no longer one SELECT (the catalog
--   written directly); its backing view still reads: writes refresh it in full.
-- - tv_lost: upgrade_lost_owner may no longer read the table its TVIEW reads.
-- - tv_col: upgrade_lost_owner may no longer read one column its TVIEW reads,
--   which the previous release's re-derivation does not see (the operator's
--   pre-check names it).
-- - tv_rls: row security applies to upgrade_rls_owner on the table its TVIEW
--   reads, which this release refuses.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
SET search_path TO "$user", public, tviews;

DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'upgrade_lost_owner') THEN
        CREATE ROLE upgrade_lost_owner;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'upgrade_rls_owner') THEN
        CREATE ROLE upgrade_rls_owner;
    END IF;
END $$;
GRANT CREATE ON SCHEMA public TO upgrade_lost_owner, upgrade_rls_owner;

CREATE TABLE tb_kept (pk_kept int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text);
CREATE TABLE tb_forced (pk_forced int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text, note text);
CREATE TABLE tb_lost (pk_lost int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text);
CREATE TABLE tb_rls (pk_rls int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     title text);
CREATE TABLE tb_col (pk_col int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     title text);
INSERT INTO tb_kept (pk_kept, title) VALUES (1, 'k1');
INSERT INTO tb_col (pk_col, title) VALUES (1, 'c1');
INSERT INTO tb_forced (pk_forced, name) VALUES (1, 'f1'), (2, 'f2');
INSERT INTO tb_lost (pk_lost, title) VALUES (1, 'l1');
INSERT INTO tb_rls (pk_rls, title) VALUES (1, 'r1');

SELECT pg_tviews_create('tv_kept', $$
    SELECT pk_kept, id, jsonb_build_object('title', title) AS data FROM tb_kept $$);
-- name is copied (a patch), note is read by nothing (a skip).
SELECT pg_tviews_create('tv_forced', $$
    SELECT pk_forced, id, jsonb_build_object('name', name) AS data FROM tb_forced $$);
UPDATE tviews.pg_tview_meta SET definition = definition || '; SELECT 1' WHERE entity = 'forced';

GRANT SELECT, TRIGGER ON tb_lost TO upgrade_lost_owner;
SET ROLE upgrade_lost_owner;
SELECT pg_tviews_create('tv_lost', $$
    SELECT pk_lost, id, jsonb_build_object('title', title) AS data FROM public.tb_lost $$);
RESET ROLE;
REVOKE SELECT ON tb_lost FROM upgrade_lost_owner;

GRANT SELECT, TRIGGER ON tb_col TO upgrade_lost_owner;
SET ROLE upgrade_lost_owner;
SELECT pg_tviews_create('tv_col', $$
    SELECT pk_col, id, jsonb_build_object('title', title) AS data FROM public.tb_col $$);
RESET ROLE;
REVOKE SELECT ON tb_col FROM upgrade_lost_owner;
GRANT SELECT (pk_col, id) ON tb_col TO upgrade_lost_owner;

GRANT SELECT, TRIGGER ON tb_rls TO upgrade_rls_owner;
SET ROLE upgrade_rls_owner;
SELECT pg_tviews_create('tv_rls', $$
    SELECT pk_rls, id, jsonb_build_object('title', title) AS data FROM public.tb_rls $$);
RESET ROLE;
CREATE POLICY rls_all ON tb_rls USING (true);
ALTER TABLE tb_rls ENABLE ROW LEVEL SECURITY;
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM tviews.pg_tview_meta
                   WHERE entity = 'forced' AND definition LIKE '%; SELECT 1') THEN
        RAISE EXCEPTION 'upgrade fixture: tv_forced''s definition was not changed';
    END IF;
END $$;

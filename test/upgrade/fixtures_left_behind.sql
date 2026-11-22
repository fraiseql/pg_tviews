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
-- - tv_shelf: aggregates its books in an order nothing fixes, which this
--   release refuses (the order rule); the operator's pre-check cannot tell, the
--   rehearsal of the update names it.

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
CREATE TABLE tb_shelf (pk_shelf int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       name text);
CREATE TABLE tb_book (pk_book int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_shelf int NOT NULL REFERENCES tb_shelf, title text);
CREATE INDEX ON tb_book (fk_shelf);
INSERT INTO tb_shelf (pk_shelf, name) VALUES (1, 's1');
INSERT INTO tb_book (pk_book, fk_shelf, title) VALUES (1, 1, 'b1');

SELECT pg_tviews_create('tv_kept', $$
    SELECT pk_kept, id, jsonb_build_object('title', title) AS data FROM tb_kept $$);
SELECT pg_tviews_create('tv_shelf', $$
    SELECT s.pk_shelf, s.id, jsonb_build_object('name', s.name,
        'books', (SELECT jsonb_agg(b.title) FROM tb_book b WHERE b.fk_shelf = s.pk_shelf)) AS data
    FROM tb_shelf s $$);
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

-- A release that refuses row security applying to a role a TVIEW reads as also
-- refuses the statements making it apply, but sees none made from another
-- database: tv_rls's owner is exempt (BYPASSRLS) while the policy is set up
-- here, and loses the exemption from the postgres database.
GRANT SELECT, TRIGGER ON tb_rls TO upgrade_rls_owner;
ALTER ROLE upgrade_rls_owner BYPASSRLS;
SET ROLE upgrade_rls_owner;
SELECT pg_tviews_create('tv_rls', $$
    SELECT pk_rls, id, jsonb_build_object('title', title) AS data FROM public.tb_rls $$);
RESET ROLE;
CREATE POLICY rls_all ON tb_rls USING (true);
ALTER TABLE tb_rls ENABLE ROW LEVEL SECURITY;
\set fixture_db :DBNAME
\c postgres
ALTER ROLE upgrade_rls_owner NOBYPASSRLS;
\c :fixture_db
SET client_min_messages TO WARNING;
SET search_path TO "$user", public, tviews;
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM tviews.pg_tview_meta
                   WHERE entity = 'forced' AND definition LIKE '%; SELECT 1') THEN
        RAISE EXCEPTION 'upgrade fixture: tv_forced''s definition was not changed';
    END IF;
END $$;

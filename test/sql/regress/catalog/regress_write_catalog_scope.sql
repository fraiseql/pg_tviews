-- Regression test: a write reads the full catalog row of the TVIEWs it reaches
-- only.
--
-- The write path needs every TVIEW's plan (the cascade paths starting at the
-- written table, the dependency graph), but not every TVIEW's whole catalog row:
-- the row's collections are a subquery per TVIEW over tviews.pg_tview_references,
-- the class of SQL that made every write depend on privileges on unrelated
-- schemas (#251). The global read is the entity and the plan; the full row is
-- loaded by entity. Measured as scans of pg_tview_meta in the writing
-- transaction, in a fresh backend: they must not grow with the TVIEWs elsewhere.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_write_catalog_scope.sql
--
-- expect-output: regress_write_catalog_scope: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS pa CASCADE;
DROP SCHEMA IF EXISTS pb CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE SCHEMA pa;
CREATE TABLE pa.tb_alpha (
    pk_alpha int PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    v        int NOT NULL
);
INSERT INTO pa.tb_alpha VALUES (1, DEFAULT, 0);
SELECT tviews.pg_tviews_create('pa.tv_alpha', $$
    SELECT a.pk_alpha, a.id, jsonb_build_object('v', a.v) AS data FROM pa.tb_alpha a $$);

-- 50 TVIEWs elsewhere, each with a reference (a collection).
CREATE SCHEMA pb;
CREATE TABLE pb.tb_root (
    pk_root int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE
);
INSERT INTO pb.tb_root VALUES (1);
SELECT tviews.pg_tviews_create('pb.tv_root', $$
    SELECT r.pk_root, r.id, jsonb_build_object() AS data FROM pb.tb_root r $$);
DO $$
BEGIN
    FOR i IN 1..50 LOOP
        EXECUTE format($f$
            CREATE TABLE pb.tb_e%1$s (
                pk_e%1$s int PRIMARY KEY,
                id       uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                fk_root  int REFERENCES pb.tb_root
            );
            SELECT tviews.pg_tviews_create('pb.tv_e%1$s', 'SELECT e.pk_e%1$s, e.id, e.fk_root,
                jsonb_build_object() AS data FROM pb.tb_e%1$s e')$f$, i);
    END LOOP;
END $$;

CREATE FUNCTION public.meta_scans() RETURNS bigint LANGUAGE sql AS $$
    SELECT COALESCE(seq_scan, 0) + COALESCE(idx_scan, 0)
    FROM pg_catalog.pg_stat_xact_user_tables
    WHERE relid = 'tviews.pg_tview_meta'::regclass $$;

-- A fresh backend: nothing cached.
\c
SET client_min_messages TO WARNING;
BEGIN;
SELECT public.meta_scans() AS before \gset
UPDATE pa.tb_alpha SET v = 1 WHERE pk_alpha = 1;
SELECT public.meta_scans() - :before AS scans \gset
COMMIT;

DO $$
BEGIN
    IF (SELECT data->>'v' FROM pa.tv_alpha) IS DISTINCT FROM '1' THEN
        RAISE EXCEPTION 'tv_alpha was not refreshed: %', (SELECT data FROM pa.tv_alpha);
    END IF;
END $$;
SELECT :scans < 10 AS scoped \gset
\if :scoped
\else
    \echo 'FAIL: the write scanned pg_tview_meta' :scans 'times (51 TVIEWs elsewhere)'
    SELECT 1/0;
\endif

DROP SCHEMA pa CASCADE;
DROP SCHEMA pb CASCADE;
DROP FUNCTION public.meta_scans();

\echo 'regress_write_catalog_scope: PASS'

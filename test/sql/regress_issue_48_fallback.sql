-- Regression test for issue #48, FALLBACK path (jsonb_delta NOT installed).
--
-- On the buggy tree the fallback path drops multi-row INSERTs (the bulk refresh
-- was `UPDATE … FROM unnest()`, UPDATE-only) and leaves stale rows on DELETE.
-- Single-row INSERTs already worked in fallback (apply_full_replacement upserts),
-- so the distinguishing assertions here are the multi-row INSERT and the DELETE.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_48_fallback.sql
--
-- Deliberately does NOT create the jsonb_delta extension.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION pg_tviews;   -- no jsonb_delta on purpose

DROP TABLE IF EXISTS tv_order CASCADE;
DROP VIEW  IF EXISTS v_order CASCADE;
DROP TABLE IF EXISTS tb_order CASCADE;
DROP TABLE IF EXISTS tb_tenant CASCADE;

CREATE TABLE tb_tenant (
    pk_tenant INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name      TEXT NOT NULL
);
CREATE TABLE tb_order (
    pk_order   INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    identifier TEXT NOT NULL,
    fk_tenant  INTEGER NOT NULL REFERENCES tb_tenant(pk_tenant),
    status     TEXT DEFAULT 'draft'
);

SELECT pg_tviews_create('tv_order', $TVIEW$
    SELECT o.pk_order AS pk_order, o.id AS id, o.fk_tenant AS fk_tenant,
           jsonb_build_object('id', o.id::text, 'identifier', o.identifier,
                              'status', o.status, 'tenant_name', t.name) AS data
    FROM tb_order o JOIN tb_tenant t ON t.pk_tenant = o.fk_tenant
$TVIEW$);

INSERT INTO tb_tenant (name) VALUES ('T1');

-- Single-row INSERTs (already worked in fallback, kept as a guard).
INSERT INTO tb_order (identifier, fk_tenant, status) VALUES ('o1', 1, 'draft');
INSERT INTO tb_order (identifier, fk_tenant, status) VALUES ('o2', 1, 'draft');
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_order) <> 2 THEN
    RAISE EXCEPTION '#48 fallback FAIL: after two single INSERTs, count = % (expected 2)',
      (SELECT count(*) FROM tv_order);
  END IF;
END $$;

-- Multi-row INSERT — the fallback-path regression (bulk UPDATE-only dropped these).
INSERT INTO tb_order (identifier, fk_tenant, status)
  SELECT x, 1, 'draft' FROM (VALUES ('o3'),('o4'),('o5')) v(x);
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_order) <> 5 THEN
    RAISE EXCEPTION '#48 fallback FAIL: after multi-row INSERT, count = % (expected 5 — rows dropped)',
      (SELECT count(*) FROM tv_order);
  END IF;
END $$;

-- Multi-row DELETE — must remove the tview rows, not leave them stale.
DELETE FROM tb_order WHERE identifier IN ('o1', 'o3');
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM tv_order WHERE data->>'identifier' IN ('o1','o3')) THEN
    RAISE EXCEPTION '#48 fallback FAIL: stale rows remain after DELETE';
  END IF;
  IF (SELECT count(*) FROM tv_order) <> 3 THEN
    RAISE EXCEPTION '#48 fallback FAIL: after DELETE, count = % (expected 3)',
      (SELECT count(*) FROM tv_order);
  END IF;
END $$;

-- Divergence probe.
DO $$
DECLARE d bigint;
BEGIN
  SELECT count(*) INTO d
  FROM tv_order t FULL OUTER JOIN v_order v USING (pk_order)
  WHERE t.data IS DISTINCT FROM v.data;
  IF d <> 0 THEN
    RAISE EXCEPTION '#48 fallback FAIL: divergence probe = % (expected 0)', d;
  END IF;
END $$;

\echo '#48 fallback PASS: incremental refresh consistent without jsonb_delta'

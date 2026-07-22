-- Regression test for issue #48:
--   "Incremental refresh silently drops INSERT statements after the first per
--    entity; DELETE refresh fails leaving stale rows."
--
-- On the buggy tree this FAILS at the "second INSERT" assertion (jsonb_delta
-- smart-patch path emits an UPDATE-only statement that no-ops for a not-yet-
-- materialized row) and again at the DELETE assertion (refresh recomputes the
-- deleted pk from the backing view after the base row is gone, errors, swallows
-- it, and leaves a stale tview row).
--
-- Run against the real jsonb_delta extension (the recommended pairing).
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_48_incremental_refresh.sql
--
-- A companion assertion at the end runs the divergence probe: tv_order and
-- v_order must agree row-for-row.

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_order CASCADE;
DROP VIEW  IF EXISTS v_order CASCADE;
DROP TABLE IF EXISTS tb_order CASCADE;
DROP TABLE IF EXISTS tb_user CASCADE;
DROP TABLE IF EXISTS tb_tenant CASCADE;

CREATE TABLE tb_tenant (
    pk_tenant  INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    identifier TEXT NOT NULL,
    name       TEXT NOT NULL
);
CREATE TABLE tb_user (
    pk_user    INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    identifier TEXT NOT NULL,
    fk_tenant  INTEGER NOT NULL REFERENCES tb_tenant(pk_tenant),
    email      TEXT,
    full_name  TEXT
);
CREATE TABLE tb_order (
    pk_order    INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id          UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    identifier  TEXT NOT NULL,
    fk_tenant   INTEGER NOT NULL REFERENCES tb_tenant(pk_tenant),
    fk_customer INTEGER REFERENCES tb_user(pk_user),
    status      TEXT DEFAULT 'draft'
);

SELECT pg_tviews_create('tv_order', $TVIEW$
    SELECT o.pk_order AS pk_order, o.id AS id, o.fk_tenant AS fk_tenant,
           jsonb_build_object('id', o.id::text, 'identifier', o.identifier,
                              'status', o.status, 'tenant_name', t.name) AS data
    FROM tb_order o JOIN tb_tenant t ON t.pk_tenant = o.fk_tenant
$TVIEW$);

INSERT INTO tb_tenant (identifier, name) VALUES ('t1', 'T1');
INSERT INTO tb_user (identifier, fk_tenant, email, full_name)
  SELECT 'u1', pk_tenant, 'u@x.io', 'U1' FROM tb_tenant;

-- Statement 1: first INSERT (o1). Must materialize.
INSERT INTO tb_order (identifier, fk_tenant, fk_customer, status)
  SELECT 'o1', t.pk_tenant, u.pk_user, 'draft' FROM tb_tenant t JOIN tb_user u ON true;
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_order) <> 1 THEN
    RAISE EXCEPTION '#48 FAIL: after first INSERT, tv_order count = % (expected 1)',
      (SELECT count(*) FROM tv_order);
  END IF;
END $$;

-- Statement 2: second INSERT (o2) — the regression. Must also materialize.
INSERT INTO tb_order (identifier, fk_tenant, fk_customer, status)
  SELECT 'o2', t.pk_tenant, u.pk_user, 'draft' FROM tb_tenant t JOIN tb_user u ON true;
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_order) <> 2 THEN
    RAISE EXCEPTION '#48 FAIL: after second INSERT, tv_order count = % (expected 2 — o2 silently dropped)',
      (SELECT count(*) FROM tv_order);
  END IF;
END $$;

-- Statement 3: third INSERT (o3), single-row VALUES form. Must materialize.
INSERT INTO tb_order (identifier, fk_tenant, fk_customer, status)
  VALUES ('o3', 1, 1, 'draft');
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_order) <> 3 THEN
    RAISE EXCEPTION '#48 FAIL: after third INSERT, tv_order count = % (expected 3)',
      (SELECT count(*) FROM tv_order);
  END IF;
END $$;

-- Statement 4: multi-row INSERT after the first statement. All rows present.
INSERT INTO tb_order (identifier, fk_tenant, fk_customer, status)
  SELECT x, 1, 1, 'draft' FROM (VALUES ('o4'),('o5'),('o6')) v(x);
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_order) <> 6 THEN
    RAISE EXCEPTION '#48 FAIL: after multi-row INSERT, tv_order count = % (expected 6)',
      (SELECT count(*) FROM tv_order);
  END IF;
END $$;

-- Statement 5: DELETE o1. The tview row must be removed, not left stale, and no
-- SPI error may be swallowed.
DELETE FROM tb_order WHERE identifier = 'o1';
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM tv_order WHERE data->>'identifier' = 'o1') THEN
    RAISE EXCEPTION '#48 FAIL: after DELETE, stale o1 row remains in tv_order';
  END IF;
  IF (SELECT count(*) FROM tv_order) <> 5 THEN
    RAISE EXCEPTION '#48 FAIL: after DELETE, tv_order count = % (expected 5)',
      (SELECT count(*) FROM tv_order);
  END IF;
END $$;

-- Divergence probe: tv_order and v_order must agree row-for-row.
DO $$
DECLARE d bigint;
BEGIN
  SELECT count(*) INTO d
  FROM tv_order t FULL OUTER JOIN v_order v USING (pk_order)
  WHERE t.data IS DISTINCT FROM v.data;
  IF d <> 0 THEN
    RAISE EXCEPTION '#48 FAIL: divergence probe = % (expected 0)', d;
  END IF;
END $$;

\echo '#48 PASS: incremental INSERT/DELETE refresh is consistent'

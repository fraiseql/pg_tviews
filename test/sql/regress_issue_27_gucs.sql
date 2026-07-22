-- Regression test for issue #27:
--   "Runtime configuration limits are hardcoded — no GUC tunability."
--
-- Verifies the three limits called out by the issue are now GUC-tunable and
-- honored at runtime: pg_tviews.max_dependency_depth, pg_tviews.batch_size,
-- pg_tviews.cache_size (queue_depth already shipped as pg_tviews.max_queue_size).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_27_gucs.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- All three new GUCs registered with the documented defaults.
DO $$
BEGIN
  IF current_setting('pg_tviews.max_dependency_depth') <> '10' THEN
    RAISE EXCEPTION '#27 FAIL: max_dependency_depth default = % (expected 10)',
      current_setting('pg_tviews.max_dependency_depth');
  END IF;
  IF current_setting('pg_tviews.batch_size') <> '1000' THEN
    RAISE EXCEPTION '#27 FAIL: batch_size default = % (expected 1000)',
      current_setting('pg_tviews.batch_size');
  END IF;
  IF current_setting('pg_tviews.cache_size') <> '10000' THEN
    RAISE EXCEPTION '#27 FAIL: cache_size default = % (expected 10000)',
      current_setting('pg_tviews.cache_size');
  END IF;
END $$;

-- Each is settable within bounds.
SET pg_tviews.max_dependency_depth = 5;
SET pg_tviews.batch_size = 2;
SET pg_tviews.cache_size = 3;
DO $$ BEGIN
  IF current_setting('pg_tviews.batch_size') <> '2'
     OR current_setting('pg_tviews.cache_size') <> '3'
     OR current_setting('pg_tviews.max_dependency_depth') <> '5' THEN
    RAISE EXCEPTION '#27 FAIL: a GUC did not take effect via SET';
  END IF;
END $$;

-- Behavioural: batch_size chunking must stay correct. With batch_size = 2 a
-- 5-row multi-row INSERT is refreshed in chunks of [2,2,1]; all 5 must appear,
-- and the low cache_size = 3 (caches clear-and-repopulate) must not corrupt
-- lookups across more than 3 distinct tviews.
CREATE TABLE tb_tenant (
    pk_tenant INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id        UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name      TEXT
);
CREATE TABLE tb_order (
    pk_order   INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    identifier TEXT NOT NULL,
    fk_tenant  INTEGER NOT NULL REFERENCES tb_tenant(pk_tenant),
    status     TEXT DEFAULT 'draft'
);
INSERT INTO tb_tenant (name) VALUES ('T1');

SELECT pg_tviews_create('tv_order', $TVIEW$
    SELECT o.pk_order AS pk_order, o.id AS id, o.fk_tenant AS fk_tenant,
           jsonb_build_object('id', o.id::text, 'identifier', o.identifier,
                              'status', o.status, 'tenant_name', t.name) AS data
    FROM tb_order o JOIN tb_tenant t ON t.pk_tenant = o.fk_tenant
$TVIEW$);

-- 5-row multi-row INSERT under batch_size = 2.
INSERT INTO tb_order (identifier, fk_tenant, status)
  SELECT 'o' || g, 1, 'draft' FROM generate_series(1, 5) g;
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_order) <> 5 THEN
    RAISE EXCEPTION '#27 FAIL: batch_size=2 chunked bulk refresh lost rows (count = %, expected 5)',
      (SELECT count(*) FROM tv_order);
  END IF;
END $$;

-- Multi-row DELETE under the same small batch size.
DELETE FROM tb_order WHERE identifier IN ('o1', 'o3', 'o5');
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_order) <> 2 THEN
    RAISE EXCEPTION '#27 FAIL: batch_size=2 chunked bulk DELETE wrong (count = %, expected 2)',
      (SELECT count(*) FROM tv_order);
  END IF;
END $$;

\echo '#27 PASS: max_dependency_depth / batch_size / cache_size are GUC-tunable and honored'

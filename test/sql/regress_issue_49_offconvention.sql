-- Regression test for issue #49:
--   "pg_tviews_create accepts an off-convention tview (entity ≠ base table); it
--    never refreshes and shadows the sibling on the same base table."
--
-- Correct behaviour: creating a tview whose derived entity has no matching
-- tb_<entity> base table with a pk_<entity> column is rejected at create time,
-- and the incumbent, correctly-named tview keeps working.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_49_offconvention.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

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
INSERT INTO tb_tenant (name) VALUES ('T1');

-- Incumbent, correctly-named tview.
SELECT pg_tviews_create('tv_order', $TVIEW$
    SELECT o.pk_order AS pk_order, o.id AS id, o.fk_tenant AS fk_tenant,
           jsonb_build_object('id', o.id::text, 'status', o.status,
                              'tenant_name', t.name) AS data
    FROM tb_order o JOIN tb_tenant t ON t.pk_tenant = o.fk_tenant
$TVIEW$);

INSERT INTO tb_order (identifier, fk_tenant, status) VALUES ('o1', 1, 'draft');
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_order) <> 1 THEN
    RAISE EXCEPTION '#49 setup FAIL: tv_order did not materialize o1';
  END IF;
END $$;

-- Off-convention create: entity would be 'order_summary' (from pk_order_summary),
-- but there is no tb_order_summary. Must be rejected.
DO $$
BEGIN
  PERFORM pg_tviews_create('tv_order_summary', $q$
      SELECT o.pk_order AS pk_order_summary, o.id AS id,
             jsonb_build_object('id', o.id::text, 'status', o.status) AS data
      FROM tb_order o
  $q$);
  RAISE EXCEPTION '#49 FAIL: off-convention create was ACCEPTED (expected rejection)';
EXCEPTION
  WHEN OTHERS THEN
    IF SQLERRM LIKE '%#49 FAIL%' THEN
      RAISE;   -- re-raise our own assertion failure
    END IF;
    RAISE NOTICE '#49 ok: off-convention create rejected: %', SQLERRM;
END $$;

-- The off-convention tview must not have been created.
DO $$ BEGIN
  IF to_regclass('tv_order_summary') IS NOT NULL THEN
    RAISE EXCEPTION '#49 FAIL: tv_order_summary table exists after a rejected create';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'order_summary') THEN
    RAISE EXCEPTION '#49 FAIL: order_summary registered in pg_tview_meta after rejection';
  END IF;
END $$;

-- Sibling protection: the incumbent tv_order must still refresh after the rejected
-- create (both a new INSERT and an UPDATE to an existing row).
INSERT INTO tb_order (identifier, fk_tenant, status) VALUES ('o2', 1, 'draft');
UPDATE tb_order SET status = 'shipped' WHERE identifier = 'o1';
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_order) <> 2 THEN
    RAISE EXCEPTION '#49 FAIL: incumbent tv_order stopped inserting (count = %, expected 2)',
      (SELECT count(*) FROM tv_order);
  END IF;
  IF (SELECT data->>'status' FROM tv_order WHERE pk_order = 1) <> 'shipped' THEN
    RAISE EXCEPTION '#49 FAIL: incumbent tv_order stopped tracking UPDATEs (o1 status = %)',
      (SELECT data->>'status' FROM tv_order WHERE pk_order = 1);
  END IF;
END $$;

\echo '#49 PASS: off-convention create rejected; incumbent sibling unaffected'

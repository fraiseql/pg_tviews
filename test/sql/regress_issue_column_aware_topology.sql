-- Hardening: column-aware refresh across cascade topologies — multi-hop paths
-- (cascade through a non-TVIEW intermediate table) and INSERT/DELETE membership.
--
-- Chain: tb_order → tb_group → tb_item; tv_order JOINs all three and aggregates
-- items. A change to tb_item cascades through tb_group to the owning order. Each
-- source table carries an UNREFERENCED column to prove column-aware skips it even
-- for multi-hop paths, while referenced-column and membership changes still cascade.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_column_aware_topology.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_order CASCADE; DROP VIEW IF EXISTS v_order CASCADE;
DROP TABLE IF EXISTS tb_item CASCADE;
DROP TABLE IF EXISTS tb_group CASCADE;
DROP TABLE IF EXISTS tb_order CASCADE;

CREATE TABLE tb_order (
    pk_order BIGINT PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE, customer TEXT NOT NULL
);
CREATE TABLE tb_group (
    pk_group BIGINT PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_order BIGINT NOT NULL REFERENCES tb_order,
    label TEXT NOT NULL, internal_code TEXT   -- internal_code: never referenced by v_order
);
CREATE TABLE tb_item (
    pk_item BIGINT PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_group BIGINT NOT NULL REFERENCES tb_group,
    product TEXT NOT NULL, qty INTEGER NOT NULL DEFAULT 1,
    internal_note TEXT                        -- internal_note: never referenced by v_order
);

INSERT INTO tb_order (pk_order, customer) VALUES (1, 'Alice'), (2, 'Bob');
INSERT INTO tb_group (pk_group, fk_order, label) VALUES (10, 1, 'Group A1'), (20, 1, 'Group A2'), (30, 2, 'Group B1');
INSERT INTO tb_item (pk_item, fk_group, product, qty) VALUES
    (100, 10, 'Widget', 2), (101, 10, 'Gadget', 1), (102, 20, 'Bolt', 5),
    (103, 30, 'Nut', 10), (104, 30, 'Washer', 3);

SELECT pg_tviews_create('order', $TVIEW$
    SELECT o.pk_order, o.id,
        jsonb_build_object('customer', o.customer,
            'items', COALESCE(jsonb_agg(jsonb_build_object(
                'product', i.product, 'qty', i.qty, 'group', g.label))
                FILTER (WHERE i.pk_item IS NOT NULL), '[]'::jsonb)) AS data
    FROM tb_order o
    LEFT JOIN tb_group g ON g.fk_order = o.pk_order
    LEFT JOIN tb_item i ON i.fk_group = g.pk_group
    GROUP BY o.pk_order, o.id, o.customer
$TVIEW$);

CREATE FUNCTION _rc() RETURNS bigint LANGUAGE sql AS
  $$ SELECT (pg_tviews_queue_stats()->>'view_recomputes')::bigint $$;
-- Products embedded in order 1 (through the 2-hop cascade).
CREATE FUNCTION _o1_products() RETURNS text[] LANGUAGE sql AS
  $$ SELECT array_agg(x->>'product' ORDER BY x->>'product')
     FROM tv_order, jsonb_array_elements(data->'items') x WHERE pk_order = 1 $$;

-- Baseline.
DO $$ BEGIN
  IF _o1_products() <> ARRAY['Bolt','Gadget','Widget'] THEN
    RAISE EXCEPTION 'setup FAIL: order 1 products = %', _o1_products();
  END IF;
END $$;

-- ── Multi-hop, referenced leaf column (tb_item.product) → cascade ────────────
DO $$ BEGIN
  UPDATE tb_item SET product = 'Widget2' WHERE pk_item = 100;
  IF NOT (_o1_products() @> ARRAY['Widget2']) THEN
    RAISE EXCEPTION 'topology FAIL: 2-hop tb_item.product change did not cascade to tv_order (%).', _o1_products();
  END IF;
END $$;

-- ── Multi-hop, UNREFERENCED leaf column (tb_item.internal_note) → SKIP ───────
DO $$
DECLARE rc0 bigint; before text[];
BEGIN
  before := _o1_products();
  rc0 := _rc();
  UPDATE tb_item SET internal_note = 'restock' WHERE pk_item = 100;
  IF _rc() <> rc0 THEN
    RAISE EXCEPTION 'topology FAIL: unreferenced tb_item.internal_note triggered % recompute(s)', _rc() - rc0;
  END IF;
  IF _o1_products() <> before THEN
    RAISE EXCEPTION 'topology FAIL: tv_order changed after a no-op internal_note edit';
  END IF;
END $$;

-- ── Intermediate table, referenced column (tb_group.label) → cascade ────────
DO $$ BEGIN
  UPDATE tb_group SET label = 'Group A1x' WHERE pk_group = 10;
  IF (SELECT count(*) FROM tv_order, jsonb_array_elements(data->'items') x
      WHERE pk_order = 1 AND x->>'group' = 'Group A1x') < 1 THEN
    RAISE EXCEPTION 'topology FAIL: tb_group.label change did not cascade to tv_order';
  END IF;
END $$;

-- ── Intermediate table, UNREFERENCED column (tb_group.internal_code) → SKIP ──
DO $$
DECLARE rc0 bigint;
BEGIN
  rc0 := _rc();
  UPDATE tb_group SET internal_code = 'XYZ' WHERE pk_group = 10;
  IF _rc() <> rc0 THEN
    RAISE EXCEPTION 'topology FAIL: unreferenced tb_group.internal_code triggered % recompute(s)', _rc() - rc0;
  END IF;
END $$;

-- ── Membership: INSERT/DELETE a child (changed = None) → always cascade ──────
DO $$ BEGIN
  INSERT INTO tb_item (pk_item, fk_group, product, qty) VALUES (105, 10, 'Sprocket', 4);
  IF NOT (_o1_products() @> ARRAY['Sprocket']) THEN
    RAISE EXCEPTION 'topology FAIL: INSERT child did not cascade (membership): %', _o1_products();
  END IF;
  DELETE FROM tb_item WHERE pk_item = 105;
  IF _o1_products() @> ARRAY['Sprocket'] THEN
    RAISE EXCEPTION 'topology FAIL: DELETE child did not cascade (membership): %', _o1_products();
  END IF;
END $$;

-- ── Byte-identity: order 1 fast-path state == forced recompute ───────────────
DO $$
DECLARE fast jsonb; rec jsonb;
BEGIN
  SELECT data INTO fast FROM tv_order WHERE pk_order = 1;
  PERFORM pg_tviews_refresh('order');
  SELECT data INTO rec FROM tv_order WHERE pk_order = 1;
  IF fast <> rec THEN
    RAISE EXCEPTION 'topology FAIL: tv_order not byte-identical to a forced recompute';
  END IF;
END $$;

SELECT 'column-aware topology: PASS' AS result;

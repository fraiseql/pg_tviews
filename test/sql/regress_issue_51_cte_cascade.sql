-- Regression test for issue #51 (part 2/3):
--   "Changes to a base table referenced only inside a CTE do not cascade."
--
-- `extract_join_paths` never reads `query.with`, so a CTE name in the main FROM
-- is parsed as an ordinary table. `resolve_join_path` then fails to find the CTE
-- alias in the base-table oid_map and downgrades the path to a `notice!`
-- "unresolvable". The base table wrapped by the CTE gets a trigger (pg_depend
-- flattens the CTE) but no cascade path, so its changes are dropped.
--
-- Correct behaviour: a change to a base table reachable only through a CTE
-- refreshes the tview.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_51_cte_cascade.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_customer CASCADE;
DROP VIEW  IF EXISTS v_customer CASCADE;
DROP TABLE IF EXISTS tb_order CASCADE;
DROP TABLE IF EXISTS tb_customer CASCADE;

CREATE TABLE tb_customer (
    pk_customer INTEGER PRIMARY KEY,
    id          UUID DEFAULT gen_random_uuid() NOT NULL,
    name        TEXT
);
CREATE TABLE tb_order (
    pk_order    INTEGER PRIMARY KEY,
    id          UUID DEFAULT gen_random_uuid() NOT NULL,
    fk_customer INTEGER REFERENCES tb_customer(pk_customer),
    amount      NUMERIC
);
INSERT INTO tb_customer (pk_customer, name) VALUES (1, 'Acme'), (2, 'Globex');
INSERT INTO tb_order (pk_order, fk_customer, amount) VALUES (10, 1, 100), (11, 1, 50), (12, 2, 999);

-- tb_order is referenced ONLY inside the CTE; the entity base table tb_customer
-- refreshes via the direct path, but tb_order needs a cascade path resolved
-- through the CTE alias `cust_orders`.
SELECT pg_tviews_create('tv_customer', $TV$
  WITH cust_orders AS (
    SELECT fk_customer, count(*) AS n, sum(amount) AS total
    FROM tb_order GROUP BY fk_customer
  )
  SELECT c.pk_customer, c.id,
         jsonb_build_object('name', c.name,
                            'orders', COALESCE(co.n, 0),
                            'total',  COALESCE(co.total, 0)) AS data
  FROM tb_customer c
  LEFT JOIN cust_orders co ON co.fk_customer = c.pk_customer
$TV$);

-- Baseline: aggregate from tb_order materialized.
DO $$ BEGIN
  IF (SELECT (data->>'total')::numeric FROM tv_customer WHERE pk_customer = 1) <> 150 THEN
    RAISE EXCEPTION '#51 setup FAIL: customer 1 total is % (expected 150)',
      (SELECT data->>'total' FROM tv_customer WHERE pk_customer = 1);
  END IF;
END $$;

-- (1) UPDATE an order amount -> the customer's aggregate must refresh.
UPDATE tb_order SET amount = 500 WHERE pk_order = 10;
DO $$ BEGIN
  IF (SELECT (data->>'total')::numeric FROM tv_customer WHERE pk_customer = 1) <> 550 THEN
    RAISE EXCEPTION '#51 FAIL: CTE base-table UPDATE lost — total is % (expected 550)',
      (SELECT data->>'total' FROM tv_customer WHERE pk_customer = 1);
  END IF;
END $$;

-- (2) INSERT a new order -> count + total must refresh.
INSERT INTO tb_order (pk_order, fk_customer, amount) VALUES (13, 1, 25);
DO $$ BEGIN
  IF (SELECT (data->>'orders')::int FROM tv_customer WHERE pk_customer = 1) <> 3 THEN
    RAISE EXCEPTION '#51 FAIL: CTE base-table INSERT lost — orders is % (expected 3)',
      (SELECT data->>'orders' FROM tv_customer WHERE pk_customer = 1);
  END IF;
  IF (SELECT (data->>'total')::numeric FROM tv_customer WHERE pk_customer = 1) <> 575 THEN
    RAISE EXCEPTION '#51 FAIL: CTE base-table INSERT total wrong — % (expected 575)',
      (SELECT data->>'total' FROM tv_customer WHERE pk_customer = 1);
  END IF;
END $$;

-- (3) DELETE an order -> aggregate must refresh.
DELETE FROM tb_order WHERE pk_order = 12;
DO $$ BEGIN
  IF (SELECT (data->>'orders')::int FROM tv_customer WHERE pk_customer = 2) <> 0 THEN
    RAISE EXCEPTION '#51 FAIL: CTE base-table DELETE lost — customer 2 orders is % (expected 0)',
      (SELECT data->>'orders' FROM tv_customer WHERE pk_customer = 2);
  END IF;
END $$;

-- (4) WITH RECURSIVE must be rejected at create time — cascade paths cannot be
-- tracked through a recursive CTE, so it must not be created silently.
DO $$
DECLARE got_expected boolean := false;
BEGIN
  BEGIN
    PERFORM pg_tviews_create('tv_rec', $R$
      WITH RECURSIVE chain(n) AS (
        SELECT 1 UNION ALL SELECT n + 1 FROM chain WHERE n < 3
      )
      SELECT pk_customer, id, jsonb_build_object('n', 1) AS data FROM tb_customer
    $R$);
  EXCEPTION WHEN OTHERS THEN
    IF position('RECURSIVE' IN upper(SQLERRM)) > 0 THEN
      got_expected := true;
    ELSE
      RAISE EXCEPTION '#51 FAIL: WITH RECURSIVE rejected for the wrong reason: %', SQLERRM;
    END IF;
  END;
  IF NOT got_expected THEN
    RAISE EXCEPTION '#51 FAIL: WITH RECURSIVE tview was not rejected at create';
  END IF;
END $$;

SELECT '#51 PASS: CTE-wrapped base-table changes cascade; WITH RECURSIVE rejected' AS result;

-- A composite DISTINCT ON key is refused (ADR 0169, D2): a TVIEW row is one
-- entity, addressed by one key, and parents embed it through one fk_<entity>.
-- "One row per (a, b)" is an entity of its own. Before, the create failed on a
-- duplicate key of a primary key covering the first column only, which said
-- nothing about why.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/distinct_on/regress_distinct_on_composite.sql
--
-- expect-output: composite DISTINCT ON refused: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_stock (pk_stock int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid DEFAULT gen_random_uuid() NOT NULL, sku text NOT NULL, warehouse text NOT NULL,
  qty int, taken_at int);
INSERT INTO tb_stock (sku, warehouse, qty, taken_at)
  VALUES ('a', 'w1', 1, 1), ('a', 'w2', 2, 1), ('a', 'w1', 3, 2);

DO $$
DECLARE msg text;
BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_create('tv_stock', $q$
          SELECT DISTINCT ON (s.sku, s.warehouse) s.pk_stock, s.id, s.sku, s.warehouse,
                 jsonb_build_object('qty', s.qty) AS data
          FROM tb_stock s ORDER BY s.sku, s.warehouse, s.taken_at DESC $q$);
        RAISE EXCEPTION 'composite FAIL: a composite DISTINCT ON key was accepted';
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT;
    END;
    IF msg LIKE 'composite FAIL%' THEN RAISE EXCEPTION '%', msg; END IF;
    IF msg NOT LIKE '%sku%' OR msg NOT LIKE '%warehouse%' OR msg NOT LIKE '%one key%'
       OR msg NOT LIKE '%entity%' THEN
        RAISE EXCEPTION 'composite FAIL: the refusal does not name the keys and the modelling: %', msg;
    END IF;
END $$;
DO $$ BEGIN
    IF to_regclass('tv_stock') IS NOT NULL OR to_regclass('v_stock') IS NOT NULL THEN
        RAISE EXCEPTION 'composite FAIL: the refused create left objects behind';
    END IF;
END $$;

\echo 'composite DISTINCT ON refused: PASS'

-- Regression test for issue #187: a table read under window functions that are
-- all partitioned by a column linked to the TVIEW key maps through that column,
-- as DISTINCT ON does: a write changes only the partitions it leaves and enters.
-- A window without PARTITION BY, or partitioned by an unlinked column, stays
-- untraceable, and so does a window in the top-level SELECT.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_window_partition.sql
--
-- expect-output: issue #187 window partition: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#187 FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'created';
EXCEPTION WHEN OTHERS THEN RETURN SQLERRM; END $$;
CREATE FUNCTION kinds() RETURNS text LANGUAGE sql AS $$
    SELECT cascade_kinds::text || ' uncascaded=' || uncascaded_tables::text
    FROM tviews.registry WHERE entity = 'customer' $$;

CREATE TABLE tb_label (pk_label bigint PRIMARY KEY, rank int NOT NULL);
CREATE TABLE tb_customer (pk_customer bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, fk_customer bigint REFERENCES tb_customer,
                       fk_label bigint REFERENCES tb_label,
                       placed_on date, label text, deleted_at timestamptz,
                       id uuid NOT NULL DEFAULT gen_random_uuid());
INSERT INTO tb_label VALUES (1, 1), (2, 2);
INSERT INTO tb_customer (pk_customer, name) VALUES (1, 'a'), (2, 'b'), (3, 'c');
INSERT INTO tb_order VALUES (10, 1, 1, '2026-01-01', 'first-a', NULL), (11, 1, 2, '2026-02-01', 'second-a', NULL),
                            (20, 2, 1, '2026-01-05', 'first-b', NULL);

-- The first live order per customer, with ROW_NUMBER (the issue's view) ...
CREATE VIEW v_first_order_rn AS
SELECT fk_customer, label FROM (
  SELECT o.fk_customer, o.label,
         ROW_NUMBER() OVER (PARTITION BY o.fk_customer ORDER BY o.placed_on, o.pk_order) AS rn
  FROM tb_order o WHERE o.deleted_at IS NULL) s
WHERE rn = 1;
-- ... with ranking functions over two windows of the same partition ...
CREATE VIEW v_first_order_ranks AS
SELECT fk_customer, label, last_label FROM (
  SELECT o.fk_customer, o.label,
         RANK() OVER w AS r, DENSE_RANK() OVER w AS dr,
         FIRST_VALUE(o.label) OVER (PARTITION BY o.fk_customer ORDER BY o.placed_on DESC) AS last_label
  FROM tb_order o WHERE o.deleted_at IS NULL
  WINDOW w AS (PARTITION BY o.fk_customer ORDER BY o.placed_on, o.pk_order)) s
WHERE r = 1 AND dr = 1;
-- ... and ordered by a column of a joined table.
CREATE VIEW v_first_order_by_label AS
SELECT fk_customer, label FROM (
  SELECT o.fk_customer, o.label,
         ROW_NUMBER() OVER (PARTITION BY o.fk_customer ORDER BY l.rank, o.pk_order) AS rn
  FROM tb_order o JOIN tb_label l ON l.pk_label = o.fk_label WHERE o.deleted_at IS NULL) s
WHERE rn = 1;

\set over 'SELECT c.pk_customer, c.id, c.name, f.label AS first_order FROM tb_customer c LEFT JOIN %s f ON f.fk_customer = c.pk_customer'

-- Writes that move the first row of a partition, between partitions, in and out.
CREATE FUNCTION exercise(tag text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_order SET deleted_at = now() WHERE pk_order = 10;   -- customer 1's first order goes
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': soft delete of a first row');
    UPDATE tb_order SET fk_customer = 3 WHERE pk_order = 11;      -- moves to customer 3
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': a row moved between partitions');
    INSERT INTO tb_order VALUES (21, 2, 2, '2025-12-31', 'earlier-b', NULL);
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': an earlier row inserted');
    DELETE FROM tb_order WHERE pk_order = 21;
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': a first row deleted');
    UPDATE tb_order SET deleted_at = NULL, fk_customer = 1 WHERE pk_order IN (10, 11);
    PERFORM assert_fresh('tv_customer', 'pk_customer', tag || ': rows restored');
END $$;

-- 1. ROW_NUMBER: created under the default policy, tb_order local, kept fresh.
SELECT tviews.pg_tviews_create('tv_customer', format(:'over', 'v_first_order_rn'));
SELECT must((SELECT cascade_kinds ->> 'tb_order' = 'local' AND uncascaded_tables = '{}'
             FROM tviews.registry WHERE entity = 'customer'), 'ROW_NUMBER: ' || kinds());
SELECT must((SELECT first_order FROM tv_customer WHERE pk_customer = 1) = 'first-a', 'initial rows');
SELECT exercise('ROW_NUMBER');
SELECT must((SELECT first_order FROM tv_customer WHERE pk_customer = 1) = 'first-a', 'restored rows');
SELECT tviews.pg_tviews_drop('tv_customer');

-- 2. RANK, DENSE_RANK and FIRST_VALUE over windows of the same partition.
SELECT tviews.pg_tviews_create('tv_customer', format(:'over', 'v_first_order_ranks'));
SELECT must((SELECT cascade_kinds ->> 'tb_order' = 'local' FROM tviews.registry WHERE entity = 'customer'),
            'ranks: ' || kinds());
SELECT exercise('ranks');
SELECT tviews.pg_tviews_drop('tv_customer');

-- 3. A table joined under the window maps through the join.
SELECT tviews.pg_tviews_create('tv_customer', format(:'over', 'v_first_order_by_label'));
SELECT must((SELECT cascade_kinds ->> 'tb_order' = 'local' AND cascade_kinds ->> 'tb_label' = 'mapped'
             AND uncascaded_tables = '{}' FROM tviews.registry WHERE entity = 'customer'),
            'joined under the window: ' || kinds());
SELECT exercise('joined');
UPDATE tb_label SET rank = 3 WHERE pk_label = 1;            -- order 11 becomes customer 1's first
SELECT assert_fresh('tv_customer', 'pk_customer', 'a write to the joined table');
SELECT must((SELECT first_order FROM tv_customer WHERE pk_customer = 1) = 'second-a', 'reordered by tb_label');
SELECT tviews.pg_tviews_drop('tv_customer');

-- 4. Controls: refused under the default policy, with the window as the reason.
-- Read through an aggregate, so each customer gets one row.
\set over_agg 'SELECT c.pk_customer, c.id, c.name, (SELECT max(f.label) FROM %s f WHERE f.fk_customer = c.pk_customer) AS first_order FROM tb_customer c'
CREATE VIEW v_unpartitioned AS
SELECT fk_customer, label FROM (
  SELECT o.fk_customer, o.label, count(*) OVER () AS n FROM tb_order o) s WHERE n > 0;
CREATE VIEW v_unlinked_partition AS
SELECT fk_customer, label FROM (
  SELECT o.fk_customer, o.label, ROW_NUMBER() OVER (PARTITION BY o.label ORDER BY o.pk_order) AS rn
  FROM tb_order o) s WHERE rn = 1;
CREATE VIEW v_one_unpartitioned AS
SELECT fk_customer, label FROM (
  SELECT o.fk_customer, o.label,
         ROW_NUMBER() OVER (PARTITION BY o.fk_customer ORDER BY o.pk_order) AS rn,
         count(*) OVER () AS n
  FROM tb_order o) s WHERE rn = 1 AND n > 0;
SELECT must(outcome LIKE '%public.tb_order%window function%', control || ': ' || outcome)
FROM (SELECT v AS control, error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_customer',
                                           format(:'over_agg', v))) AS outcome
      FROM unnest(ARRAY['v_unpartitioned', 'v_unlinked_partition', 'v_one_unpartitioned']) v) c;
-- A window in the top-level SELECT: its value comes from other TVIEW rows.
SELECT must(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_order',
    'SELECT o.pk_order, o.id, o.label, ROW_NUMBER() OVER (PARTITION BY o.fk_customer ORDER BY o.pk_order) AS rn FROM tb_order o'))
            LIKE '%window function in the top-level SELECT%', 'a top-level window was not refused');

SELECT 'issue #187 window partition: PASS' AS result;

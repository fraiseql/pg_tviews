-- Regression test for issue #189: a TVIEW reading a materialized view. No
-- trigger reaches a matview's rows (REFRESH MATERIALIZED VIEW rewrites them), so
-- the read is uncascaded: refused under the `error` policy, warned under `warn`,
-- and under `full_refresh` a REFRESH MATERIALIZED VIEW rebuilds the TVIEW.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/policy/regress_materialized_view.sql
--
-- expect-output: issue #189 matview: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#189 FAIL: %', what; END IF; END $$;
-- The error a statement raises, NULL if none.
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLERRM; END $$;

CREATE TABLE tb_customer (pk_customer bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, fk_customer bigint REFERENCES tb_customer, deleted_at timestamptz);
CREATE MATERIALIZED VIEW mv_order_count AS
  SELECT fk_customer, count(*) AS n FROM tb_order WHERE deleted_at IS NULL GROUP BY 1;
CREATE UNIQUE INDEX ON mv_order_count (fk_customer);
INSERT INTO tb_customer (pk_customer, name) VALUES (1, 'a'), (2, 'b');

\set definition 'SELECT c.pk_customer, c.id, c.name, COALESCE(m.n, 0) AS n_orders FROM tb_customer c LEFT JOIN mv_order_count m ON m.fk_customer = c.pk_customer'

-- 1. The default policy (error) refuses it, naming the matview.
SELECT coalesce(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_customer', :'definition')), 'created') AS refusal \gset
SELECT must(:'refusal' LIKE '%public.mv_order_count%materialized view%', 'not refused: ' || :'refusal');

-- 2. warn: created, the matview listed as uncascaded.
SELECT tviews.pg_tviews_create('tv_customer', :'definition', '{"uncascaded_policy": "warn"}');
SELECT must((SELECT uncascaded_tables = '{mv_order_count}' AND cascade_kinds ->> 'mv_order_count' = 'all_keys'
             FROM tviews.registry WHERE entity = 'customer'),
            'registry: ' || (SELECT uncascaded_tables::text || ' ' || cascade_kinds::text FROM tviews.registry));
SELECT must(NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'mv_order_count'::regclass),
            'a trigger on the matview');
SELECT tviews.pg_tviews_drop('tv_customer');

-- 3. full_refresh: REFRESH MATERIALIZED VIEW rebuilds the TVIEW.
SELECT tviews.pg_tviews_create_or_replace('tv_customer', :'definition',
                                          options => '{"uncascaded_policy": "full_refresh"}');
SELECT must(NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'mv_order_count'::regclass),
            'a trigger on the matview (full_refresh)');
SELECT must((SELECT status FROM tviews.pg_tviews_health_check() WHERE component = 'triggers') = 'OK',
            'health check: ' || (SELECT message FROM tviews.pg_tviews_health_check() WHERE component = 'triggers'));

INSERT INTO tb_order VALUES (10, 1, NULL), (11, 1, NULL);
SELECT must((SELECT n_orders FROM tv_customer WHERE pk_customer = 1) = 0, 'refreshed before the REFRESH');
REFRESH MATERIALIZED VIEW mv_order_count;
SELECT assert_fresh('tv_customer', 'pk_customer', 'REFRESH MATERIALIZED VIEW');
SELECT must((SELECT n_orders FROM tv_customer WHERE pk_customer = 1) = 2, 'n_orders after REFRESH');

INSERT INTO tb_order VALUES (20, 2, NULL);
REFRESH MATERIALIZED VIEW CONCURRENTLY mv_order_count;
SELECT assert_fresh('tv_customer', 'pk_customer', 'REFRESH MATERIALIZED VIEW CONCURRENTLY');

BEGIN;
UPDATE tb_order SET deleted_at = now() WHERE pk_order = 10;
REFRESH MATERIALIZED VIEW mv_order_count;
COMMIT;
SELECT assert_fresh('tv_customer', 'pk_customer', 'REFRESH inside a transaction');
SELECT must((SELECT n_orders FROM tv_customer WHERE pk_customer = 1) = 1, 'n_orders after REFRESH in a transaction');

-- WITH NO DATA empties the matview and makes it unreadable: nothing to refresh
-- from, and the statement does not fail.
REFRESH MATERIALIZED VIEW mv_order_count WITH NO DATA;
REFRESH MATERIALIZED VIEW mv_order_count;
SELECT assert_fresh('tv_customer', 'pk_customer', 'REFRESH after WITH NO DATA');

-- A REFRESH from a function refreshes too.
CREATE FUNCTION refresh_counts() RETURNS void LANGUAGE plpgsql AS $$
BEGIN REFRESH MATERIALIZED VIEW mv_order_count; END $$;
INSERT INTO tb_order VALUES (21, 2, NULL);
SELECT refresh_counts();
SELECT assert_fresh('tv_customer', 'pk_customer', 'REFRESH from a function');
SELECT tviews.pg_tviews_drop('tv_customer');

-- 4. A matview read through a plain view is the same read.
CREATE VIEW v_order_count AS SELECT fk_customer, n FROM mv_order_count;
SELECT coalesce(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_customer',
    replace(:'definition', 'mv_order_count', 'v_order_count'))), 'created') AS refusal \gset
SELECT must(:'refusal' LIKE '%public.mv_order_count%', 'through a view, not refused: ' || :'refusal');

SELECT 'issue #189 matview: PASS' AS result;

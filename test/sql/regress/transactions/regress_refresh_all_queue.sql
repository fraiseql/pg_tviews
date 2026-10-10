-- Regression test for issue #202: pg_tviews_refresh_all() and pg_tviews_refresh()
-- rebuild TVIEWs whose tables other TVIEWs read (#191). The rebuild's writes queue
-- refreshes of the readers; nothing may be left queued when the call returns, so
-- the transaction's COMMIT does not fail with "queued refreshes for".
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_refresh_all_queue.sql
--
-- expect-output: issue #202 refresh_all queue: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#202 FAIL: %', what; END IF; END $$;
CREATE FUNCTION queued() RETURNS int LANGUAGE sql AS $$
    SELECT jsonb_array_length(tviews.pg_tviews_debug_queue()) $$;

CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_line (pk_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_order bigint REFERENCES tb_order, amount numeric);
INSERT INTO tb_order VALUES (1, default, 'o1'), (2, default, 'o2');
INSERT INTO tb_line VALUES (10, default, 1, 5), (11, default, 1, 7), (12, default, 2, 1);
CREATE VIEW v_line AS SELECT l.pk_line, l.id, o.id AS order_id, l.amount
  FROM tb_line l JOIN tb_order o ON o.pk_order = l.fk_order;
SELECT tviews.pg_tviews_create('tv_line', 'SELECT pk_line, id, order_id, amount FROM public.v_line');
-- A second TVIEW reads the first one's table through an aggregating view (#191).
CREATE VIEW v_order_lines AS SELECT order_id, sum(amount) AS total FROM public.tv_line GROUP BY order_id;
SELECT tviews.pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, o.name, ol.total
  FROM public.tb_order o LEFT JOIN public.v_order_lines ol ON ol.order_id = o.id $$);

-- Rows the triggers did not see, so the rebuilds change something.
SET session_replication_role = replica;
UPDATE tb_line SET amount = amount * 10;
RESET session_replication_role;

BEGIN;
SELECT tviews.pg_tviews_refresh_all();
SELECT must(queued() = 0, format('pg_tviews_refresh_all() left %s refreshes queued', queued()));
SELECT assert_fresh('tv_line', 'pk_line', 'refresh_all');
SELECT assert_fresh('tv_order', 'pk_order', 'refresh_all');
COMMIT;

SET session_replication_role = replica;
UPDATE tb_line SET amount = amount + 1;
RESET session_replication_role;

BEGIN;
SELECT tviews.pg_tviews_refresh('line');
SELECT must(queued() = 0, format('pg_tviews_refresh(''line'') left %s refreshes queued', queued()));
SELECT assert_fresh('tv_order', 'pk_order', 'refresh of the read TVIEW');
COMMIT;

SELECT 'issue #202 refresh_all queue: PASS' AS result;

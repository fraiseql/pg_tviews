-- A fan-out patch two joins away (line -> order -> customer) finds the rows it
-- patches as the TVIEW's owner, as every other refresh read does, and locks no
-- row of the table in between (#268).
--
-- The lookup through the table in between ran as the writer and locked the
-- orders FOR SHARE: a writer allowed to update tb_customer alone failed on
-- "permission denied for table tb_order" (FOR SHARE takes UPDATE), and with a
-- policy on tb_order it saw only its own orders and left the others' lines
-- stale. It now takes shared value locks on the orders it reached (ADR 0207).
--
-- What the owner reads past the writer's policy stays with the owner: the
-- orders and lines the writer cannot see carry marker values, and none of them
-- may reach the writer's output, at any message level.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_fanout_via_as_owner.sql
--
-- reject-output: 7770010
-- reject-output: 7770011
-- reject-output: hidden-sku
-- reject-output: hidden-owner
-- reject-output: permission denied

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP TABLE IF EXISTS tb_line, tb_order, tb_customer;
DROP ROLE IF EXISTS regress_via_writer;
CREATE ROLE regress_via_writer;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'fan-out via FAIL: %', what; END IF; END $$;

CREATE TABLE tb_customer (pk_customer bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_customer bigint NOT NULL, owner_role name NOT NULL);
CREATE TABLE tb_line (pk_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_order bigint NOT NULL, sku text);
INSERT INTO tb_customer (pk_customer, name) VALUES (1, 'ann');
INSERT INTO tb_order (pk_order, fk_customer, owner_role)
    VALUES (7770010, 1, 'hidden-owner'), (7770011, 1, 'hidden-owner');
INSERT INTO tb_line (pk_line, fk_order, sku)
    VALUES (100, 7770010, 'hidden-sku-a'), (101, 7770011, 'hidden-sku-b');
SELECT tviews.pg_tviews_create('tv_line', $$
    SELECT l.pk_line, l.id, l.fk_order,
           jsonb_build_object('sku', l.sku, 'customer', c.name) AS data
    FROM tb_line l JOIN tb_order o ON o.pk_order = l.fk_order
    JOIN tb_customer c ON c.pk_customer = o.fk_customer $$) IS NOT NULL;
SELECT must((SELECT plan->'tables' @? '$[*].fanout.via' FROM tviews.pg_tview_meta
             WHERE entity = 'line'), 'tv_line has no two-hop fan-out: the test reads nothing');

-- 1. A writer of tb_customer alone, with no privilege on tb_order.
GRANT SELECT, UPDATE ON tb_customer TO regress_via_writer;
SET ROLE regress_via_writer;
SET client_min_messages TO DEBUG5;
UPDATE tb_customer SET name = 'anne' WHERE pk_customer = 1;
SET client_min_messages TO WARNING;
RESET ROLE;
SELECT must((SELECT count(*) FROM tv_line WHERE data->>'customer' = 'anne') = 2,
            'not every line of the renamed customer shows the new name');

-- 2. A policy on tb_order that shows the writer none of the orders: the lines of
-- the orders it cannot see are refreshed all the same.
GRANT SELECT, UPDATE ON tb_order TO regress_via_writer;
ALTER TABLE tb_order ENABLE ROW LEVEL SECURITY;
CREATE POLICY own_orders ON tb_order USING (owner_role = current_user);
SET ROLE regress_via_writer;
SELECT must((SELECT count(*) FROM tb_order) = 0, 'the writer sees orders: the policy hides nothing');
SET client_min_messages TO DEBUG5;
UPDATE tb_customer SET name = 'annie' WHERE pk_customer = 1;
SET client_min_messages TO WARNING;
RESET ROLE;
SELECT must((SELECT count(*) FROM tv_line WHERE data->>'customer' = 'annie') = 2,
            'a line whose order the writer cannot see kept the old name');

-- 3. The lookup locks no row of tb_order and no relation lock beyond a read:
-- what serializes it against a concurrent re-point is a shared value lock on
-- each order it reached.
BEGIN;
SET LOCAL ROLE regress_via_writer;
UPDATE tb_customer SET name = 'anna' WHERE pk_customer = 1;
RESET ROLE;
SELECT must(NOT EXISTS (SELECT FROM pg_locks
                        WHERE pid = pg_backend_pid() AND locktype = 'relation'
                          AND relation = 'tb_order'::regclass
                          AND mode <> 'AccessShareLock'),
            'the write took a row-locking lock on tb_order');
SELECT must(NOT EXISTS (SELECT FROM tb_order WHERE xmax::text = pg_current_xact_id()::text),
            'the write locked rows of tb_order');
SELECT must((SELECT count(*) FROM pg_locks
             WHERE pid = pg_backend_pid() AND locktype = 'advisory'
               AND classid = 'tb_order'::regclass AND objsubid = 21622  -- 0x5476
               AND mode = 'ShareLock') = 2,
            'the write did not lock the two orders it reached');
COMMIT;
SELECT must((SELECT count(*) FROM tv_line WHERE data->>'customer' = 'anna') = 2,
            'the third rename did not reach every line');

-- 4. The writer's own report of the write names no hidden value either.
SET ROLE regress_via_writer;
BEGIN;
UPDATE tb_customer SET name = 'annabel' WHERE pk_customer = 1;
SELECT must(r::text NOT LIKE '%7770010%' AND r::text NOT LIKE '%7770011%'
            AND r::text NOT LIKE '%hidden%', 'the report names a hidden value')
  FROM tviews.pg_tviews_flush_and_report(include_data => false) r;
COMMIT;
RESET ROLE;

DROP TABLE tv_line;
DROP TABLE tb_line, tb_order, tb_customer;
DROP ROLE regress_via_writer;

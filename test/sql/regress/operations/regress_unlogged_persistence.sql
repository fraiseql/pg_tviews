-- An UNLOGGED TVIEW reset by PostgreSQL (#214) is filled before anything makes its
-- emptiness permanent or blocks its writers (#215):
--   - PREPARE TRANSACTION of a transaction that claimed its refill is refused: the
--     claim would block every writer of the TVIEW until COMMIT PREPARED;
--   - ALTER TABLE … SET LOGGED, raw or through the `logged` option, fills it first:
--     a LOGGED table is never checked again.
-- Deleting the TVIEW's row in tviews.pg_tview_valid stands in for the reset.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_unlogged_persistence.sql
-- expect-output: unlogged_persistence: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

SELECT current_setting('max_prepared_transactions')::int = 0 AS no_2pc \gset
\if :no_2pc
  \echo 'SKIP: max_prepared_transactions = 0'
  \quit
\endif

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'unlogged_persistence FAIL: %', what; END IF; END $$;

CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_item VALUES (1, DEFAULT, 'a'), (2, DEFAULT, 'b');
SELECT tviews.pg_tviews_create_or_replace('tv_item',
    $$SELECT pk_item, id, jsonb_build_object('name', name) AS data FROM tb_item$$,
    '{"logged": false}');

-- 1. A transaction that claimed the refill cannot be prepared.
TRUNCATE tv_item;
DELETE FROM tviews.pg_tview_valid;
\c
SET client_min_messages TO WARNING;
BEGIN;
UPDATE tb_item SET name = 'a!' WHERE pk_item = 1;
\set ON_ERROR_STOP off
PREPARE TRANSACTION 'claimed_refill';
\set ON_ERROR_STOP on
SELECT must(:'LAST_ERROR_SQLSTATE' = '25000',
            'PREPARE with a claimed refill: ' || :'LAST_ERROR_SQLSTATE' || ' ' || :'LAST_ERROR_MESSAGE');
SELECT must(NOT EXISTS (SELECT 1 FROM pg_prepared_xacts WHERE gid = 'claimed_refill'),
            'the transaction holding the claim was prepared');
-- Nothing was kept: the next write fills it, and a transaction without a claim
-- prepares as before.
UPDATE tb_item SET name = 'a!' WHERE pk_item = 1;
SELECT assert_fresh('tv_item', 'pk_item', 'the write after the refused PREPARE');
BEGIN;
UPDATE tb_item SET name = 'b!' WHERE pk_item = 2;
PREPARE TRANSACTION 'no_claim';
COMMIT PREPARED 'no_claim';
SELECT assert_fresh('tv_item', 'pk_item', 'a prepared transaction without a claim');

-- 2. A raw ALTER TABLE … SET LOGGED on a reset TVIEW fills it first.
TRUNCATE tv_item;
DELETE FROM tviews.pg_tview_valid;
ALTER TABLE tv_item SET LOGGED;
SELECT assert_fresh('tv_item', 'pk_item', 'a reset TVIEW after a raw SET LOGGED');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tview_valid), 'a LOGGED TVIEW kept its row');

-- 3. A raw SET UNLOGGED records its rows as trusted.
ALTER TABLE tv_item SET UNLOGGED;
SELECT must(EXISTS (SELECT 1 FROM tviews.pg_tview_valid WHERE table_oid = 'tv_item'::regclass),
            'a TVIEW switched to UNLOGGED by ALTER TABLE has no row');

-- 4. The `logged` option does the same as the raw statement.
TRUNCATE tv_item;
DELETE FROM tviews.pg_tview_valid;
SELECT must(tviews.pg_tviews_create_or_replace('tv_item',
    $$SELECT pk_item, id, jsonb_build_object('name', name) AS data FROM tb_item$$,
    '{"logged": true}') = 'altered', 'the logged option did not alter the TVIEW');
SELECT assert_fresh('tv_item', 'pk_item', 'a reset TVIEW switched to LOGGED by the option');
SELECT tviews.pg_tviews_create_or_replace('tv_item',
    $$SELECT pk_item, id, jsonb_build_object('name', name) AS data FROM tb_item$$,
    '{"logged": false}');
SELECT must(EXISTS (SELECT 1 FROM tviews.pg_tview_valid WHERE table_oid = 'tv_item'::regclass),
            'a TVIEW switched to UNLOGGED by the option has no row');

\echo 'unlogged_persistence: PASS'

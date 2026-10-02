-- A TVIEW whose own table (tb_<entity>) is partitioned refreshes on writes, whether
-- they go through the root or straight to a partition or a sub-partition. The row
-- trigger PostgreSQL clones onto each partition must resolve the entity from the
-- partition root, not from the partition.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_partitioned_entity_table.sql
--
-- expect-output: partitioned entity table: PASS
-- reject-output: not managed by pg_tviews

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_item (pk_item bigint NOT NULL, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text, qty int NOT NULL DEFAULT 0,
                      PRIMARY KEY (pk_item)) PARTITION BY RANGE (pk_item);
CREATE TABLE tb_item_1 PARTITION OF tb_item FOR VALUES FROM (0) TO (100);
-- The second leaf is itself partitioned.
CREATE TABLE tb_item_2 PARTITION OF tb_item FOR VALUES FROM (100) TO (200)
    PARTITION BY RANGE (pk_item);
CREATE TABLE tb_item_2a PARTITION OF tb_item_2 FOR VALUES FROM (100) TO (150);
CREATE TABLE tb_item_2b PARTITION OF tb_item_2 FOR VALUES FROM (150) TO (200);

INSERT INTO tb_item (pk_item, name, qty) VALUES (1, 'a', 1), (2, 'b', 2), (120, 'c', 3), (170, 'd', 4);

SELECT pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, jsonb_build_object('name', i.name, 'qty', i.qty) AS data
    FROM tb_item i $$);

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_item t FULL JOIN v_item v USING (pk_item)
               WHERE t.pk_item IS NULL OR v.pk_item IS NULL
                  OR t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION 'N1 FAIL [%]: tv_item diverges from v_item', label;
    END IF;
END $$;

-- ── through the root ────────────────────────────────────────────────────────
BEGIN;
UPDATE tb_item SET name = 'A' WHERE pk_item = 1;
COMMIT;
SELECT check_fresh('UPDATE through the root');
BEGIN;
INSERT INTO tb_item (pk_item, name) VALUES (3, 'new'), (130, 'new sub');
COMMIT;
SELECT check_fresh('INSERT through the root');
BEGIN;
DELETE FROM tb_item WHERE pk_item = 2;
COMMIT;
SELECT check_fresh('DELETE through the root');
BEGIN;
UPDATE tb_item SET pk_item = 160 WHERE pk_item = 3;   -- moves to another leaf
COMMIT;
SELECT check_fresh('UPDATE across partitions');

-- ── straight to a leaf and a sub-leaf ───────────────────────────────────────
BEGIN;
UPDATE tb_item_1 SET qty = 10 WHERE pk_item = 1;
COMMIT;
SELECT check_fresh('UPDATE of a leaf');
BEGIN;
UPDATE tb_item_2a SET name = 'C' WHERE pk_item = 120;
COMMIT;
SELECT check_fresh('UPDATE of a sub-leaf');
BEGIN;
INSERT INTO tb_item_2b (pk_item, name) VALUES (180, 'leaf insert');
DELETE FROM tb_item_2b WHERE pk_item = 170;
COMMIT;
SELECT check_fresh('INSERT and DELETE on a sub-leaf');

\echo 'partitioned entity table: PASS'

-- A definition that reads tableoid (the partition a row is in) stays fresh when
-- an UPDATE moves the row to another partition: the partition is a column the
-- row's document reads.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_tableoid_cross_partition.sql
--
-- expect-output: tableoid across partitions: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_m (pk_m int NOT NULL, id uuid NOT NULL DEFAULT gen_random_uuid(), region int NOT NULL,
                   name text, PRIMARY KEY (pk_m, region)) PARTITION BY LIST (region);
CREATE TABLE tb_m_1 PARTITION OF tb_m FOR VALUES IN (1);
CREATE TABLE tb_m_2 PARTITION OF tb_m FOR VALUES IN (2);
CREATE TABLE tb_o (pk_o int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_m int NOT NULL, title text);
INSERT INTO tb_m (pk_m, region, name) VALUES (1, 1, 'a'), (2, 2, 'b');
INSERT INTO tb_o (pk_o, fk_m, title) VALUES (1, 1, 'o1'), (2, 2, 'o2');
SELECT tviews.pg_tviews_create('tv_o', $$
    SELECT o.pk_o, o.id, jsonb_build_object('title', o.title, 'shelf', m.tableoid::regclass::text) AS data
    FROM tb_o o JOIN tb_m m ON m.pk_m = o.fk_m $$) IS NOT NULL AS created;

UPDATE tb_m SET region = 2 WHERE pk_m = 1;
SELECT assert_fresh('tv_o', 'pk_o', 'an UPDATE moving a row to another partition');
UPDATE tb_m SET name = 'b2' WHERE pk_m = 2;
SELECT assert_fresh('tv_o', 'pk_o', 'an UPDATE inside one partition');

SELECT 'tableoid across partitions: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;

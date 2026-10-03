-- #175: a TVIEW whose root table is not named tb_<entity> lost every write to it.
-- The row trigger found a TVIEW's own table by stripping `tb_` from the written
-- table's name and read pk_<entity> off the row by name; the cascade paths leave
-- the root table out. The root table's writes now map through its key mapping,
-- like any other table's.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_175_root_table_name.sql
--
-- expect-output: #175 root table not named tb_<entity>: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE TABLE tb_customer (pk_customer int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_order (pk_order int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), fk_customer int, ref text);
INSERT INTO tb_customer (name) VALUES ('c1'), ('c2');
INSERT INTO tb_order (fk_customer, ref) VALUES (1, 'r1'), (2, 'r2');
SELECT pg_tviews_create('tv_purchase', $$
  SELECT o.pk_order AS pk_purchase, o.id, jsonb_build_object('ref', o.ref, 'c', c.name) AS data
  FROM tb_order o JOIN tb_customer c ON c.pk_customer = o.fk_customer $$);

UPDATE tb_order SET ref = 'r1b' WHERE pk_order = 1;
SELECT assert_fresh('tv_purchase', 'pk_purchase', 'an UPDATE of the root table');
UPDATE tb_order SET ref = ref || '!';
SELECT assert_fresh('tv_purchase', 'pk_purchase', 'a two-row UPDATE of the root table');
INSERT INTO tb_order (fk_customer, ref) VALUES (1, 'r3');
SELECT assert_fresh('tv_purchase', 'pk_purchase', 'an INSERT into the root table');
DELETE FROM tb_order WHERE pk_order = 2;
SELECT assert_fresh('tv_purchase', 'pk_purchase', 'a DELETE from the root table');
UPDATE tb_customer SET name = 'c1b' WHERE pk_customer = 1;
SELECT assert_fresh('tv_purchase', 'pk_purchase', 'an UPDATE of the joined table');

\echo '#175 root table not named tb_<entity>: PASS'

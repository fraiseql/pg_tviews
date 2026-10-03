-- #172: changing the DISTINCT ON key of a row left the old group's row in the
-- TVIEW. The row trigger enqueued only the new image's key, so the group the row
-- left was never recomputed.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_distinct_on_key_change.sql
--
-- expect-output: #172 DISTINCT ON key change: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE TABLE tb_contract (pk_contract int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid DEFAULT gen_random_uuid() NOT NULL, id_contract int NOT NULL,
  version_no int NOT NULL, status text);
INSERT INTO tb_contract (id_contract, version_no, status)
  VALUES (100, 1, 'a'), (200, 1, 'a'), (200, 2, 'b'), (300, 1, 'a');
SELECT pg_tviews_create('tv_contract', $$
  SELECT DISTINCT ON (c.id_contract) c.id_contract AS pk_contract, c.id,
         jsonb_build_object('status', c.status, 'version', c.version_no) AS data
  FROM tb_contract c ORDER BY c.id_contract, c.version_no DESC $$);

-- The only row of group 300 moves to 400: 300 disappears, 400 appears.
UPDATE tb_contract SET id_contract = 400 WHERE id_contract = 300;
SELECT assert_fresh('tv_contract', 'pk_contract', 'moving a whole group to a new key');

-- The winning row of group 200 moves to group 100: 200 falls back to version 1,
-- 100 gets a new winner.
UPDATE tb_contract SET id_contract = 100 WHERE id_contract = 200 AND version_no = 2;
SELECT assert_fresh('tv_contract', 'pk_contract', 'moving a winning row to another group');

-- Several rows change key in one statement.
UPDATE tb_contract SET id_contract = id_contract + 1000;
SELECT assert_fresh('tv_contract', 'pk_contract', 'moving every row in one statement');

-- Keyed on a uuid that is not pk_<entity>.
CREATE TABLE tb_doc (pk_doc int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL, rev int NOT NULL, body text);
INSERT INTO tb_doc (id, rev, body) VALUES ('00000000-0000-0000-0000-000000000001', 1, 'a');
SELECT pg_tviews_create('tv_doc', $$
  SELECT DISTINCT ON (d.id) d.pk_doc, d.id, jsonb_build_object('body', d.body) AS data
  FROM tb_doc d ORDER BY d.id, d.rev DESC $$);
UPDATE tb_doc SET id = '00000000-0000-0000-0000-000000000009';
SELECT assert_fresh('tv_doc', 'id', 'moving a group to a new uuid');

\echo '#172 DISTINCT ON key change: PASS'

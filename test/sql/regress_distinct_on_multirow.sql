-- #171: a statement that writes rows of several DISTINCT ON groups refreshed
-- none of them. The flush took its bulk path for the entity's keys, and that path
-- dropped every DISTINCT ON key, with no warning.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_distinct_on_multirow.sql
--
-- known-failing: #171
-- expect-output: #171 multi-row writes to a DISTINCT ON TVIEW: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

-- Versioned contracts: one TVIEW row per id_contract, the latest version wins.
CREATE TABLE tb_contract (pk_contract int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid DEFAULT gen_random_uuid() NOT NULL, id_contract int NOT NULL,
  version_no int NOT NULL, status text);
INSERT INTO tb_contract (id_contract, version_no, status)
  VALUES (100, 1, 'a'), (200, 1, 'a'), (300, 1, 'a'), (300, 2, 'b');
SELECT pg_tviews_create('tv_contract', $$
  SELECT DISTINCT ON (c.id_contract) c.id_contract AS pk_contract, c.id,
         jsonb_build_object('status', c.status, 'version', c.version_no) AS data
  FROM tb_contract c ORDER BY c.id_contract, c.version_no DESC $$);

-- The same, keyed on a column that is not pk_<entity> (the row's uuid).
CREATE TABLE tb_doc (pk_doc int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL, rev int NOT NULL, body text);
INSERT INTO tb_doc (id, rev, body) VALUES
  ('00000000-0000-0000-0000-000000000001', 1, 'a'),
  ('00000000-0000-0000-0000-000000000002', 1, 'a');
SELECT pg_tviews_create('tv_doc', $$
  SELECT DISTINCT ON (d.id) d.pk_doc, d.id, jsonb_build_object('body', d.body, 'rev', d.rev) AS data
  FROM tb_doc d ORDER BY d.id, d.rev DESC $$);

UPDATE tb_contract SET status = 'u' WHERE id_contract IN (100, 200);
SELECT assert_fresh('tv_contract', 'pk_contract', 'a two-group UPDATE');

INSERT INTO tb_contract (id_contract, version_no, status) VALUES (100, 2, 'v2'), (200, 2, 'v2');
SELECT assert_fresh('tv_contract', 'pk_contract', 'a two-group INSERT of new versions');

INSERT INTO tb_contract (id_contract, version_no, status) VALUES (400, 1, 'n'), (500, 1, 'n');
SELECT assert_fresh('tv_contract', 'pk_contract', 'a two-group INSERT of new contracts');

DELETE FROM tb_contract WHERE version_no = 2 AND id_contract IN (100, 300);
SELECT assert_fresh('tv_contract', 'pk_contract', 'a two-group DELETE of the winning versions');

DELETE FROM tb_contract WHERE id_contract IN (400, 500);
SELECT assert_fresh('tv_contract', 'pk_contract', 'a two-group DELETE of whole contracts');

BEGIN;
UPDATE tb_contract SET status = 'tx1' WHERE id_contract = 100;
UPDATE tb_contract SET status = 'tx2' WHERE id_contract = 200;
COMMIT;
SELECT assert_fresh('tv_contract', 'pk_contract', 'two single-group UPDATEs in one transaction');

UPDATE tb_doc SET body = 'b';
SELECT assert_fresh('tv_doc', 'id', 'a two-group UPDATE (keyed on id)');
INSERT INTO tb_doc (id, rev, body) VALUES
  ('00000000-0000-0000-0000-000000000001', 2, 'c'),
  ('00000000-0000-0000-0000-000000000002', 2, 'c');
SELECT assert_fresh('tv_doc', 'id', 'a two-group INSERT of new revisions (keyed on id)');

\echo '#171 multi-row writes to a DISTINCT ON TVIEW: PASS'

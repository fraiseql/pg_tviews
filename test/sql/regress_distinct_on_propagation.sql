-- #173: a TVIEW embedding a DISTINCT ON TVIEW was never refreshed by changes to
-- it. Entity propagation skipped every DISTINCT ON key, so the parent kept the
-- child's old document, with no warning.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_distinct_on_propagation.sql
--
-- expect-output: #173 propagation from a DISTINCT ON TVIEW: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

-- ── child keyed on pk_<entity> (id_contract AS pk_contract) ─────────────────
CREATE TABLE tb_contract (pk_contract int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid DEFAULT gen_random_uuid() NOT NULL, id_contract int NOT NULL,
  version_no int NOT NULL, status text);
INSERT INTO tb_contract (id_contract, version_no, status)
  VALUES (100, 1, 'draft'), (100, 2, 'active'), (200, 1, 'draft');
CREATE TABLE tb_deal (pk_deal int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid DEFAULT gen_random_uuid() NOT NULL, fk_contract int NOT NULL, name text);
INSERT INTO tb_deal (fk_contract, name) VALUES (100, 'd1'), (200, 'd2');
CREATE TABLE tb_desk (pk_desk int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid DEFAULT gen_random_uuid() NOT NULL, fk_deal int NOT NULL, label text);
INSERT INTO tb_desk (fk_deal, label) VALUES (1, 'k1'), (2, 'k2');

SELECT pg_tviews_create('tv_contract', $$
  SELECT DISTINCT ON (c.id_contract) c.id_contract AS pk_contract, c.id,
         jsonb_build_object('status', c.status) AS data
  FROM tb_contract c ORDER BY c.id_contract, c.version_no DESC $$);
SELECT pg_tviews_create('tv_deal', $$
  SELECT d.pk_deal, d.id, d.fk_contract,
         jsonb_build_object('name', d.name, 'contract', vc.data) AS data
  FROM tb_deal d JOIN v_contract vc ON vc.pk_contract = d.fk_contract $$);
-- Two levels above the DISTINCT ON TVIEW.
SELECT pg_tviews_create('tv_desk', $$
  SELECT k.pk_desk, k.id, k.fk_deal,
         jsonb_build_object('label', k.label, 'deal', vd.data) AS data
  FROM tb_desk k JOIN v_deal vd ON vd.pk_deal = k.fk_deal $$);

CREATE FUNCTION check_all(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM assert_fresh('tv_contract', 'pk_contract', label);
    PERFORM assert_fresh('tv_deal', 'pk_deal', label);
    PERFORM assert_fresh('tv_desk', 'pk_desk', label);
END $$;

UPDATE tb_contract SET status = 'signed' WHERE version_no = 2;
SELECT check_all('an UPDATE of the winning version');
INSERT INTO tb_contract (id_contract, version_no, status) VALUES (200, 2, 'active');
SELECT check_all('a new winning version');
UPDATE tb_contract SET status = 'closed' WHERE id_contract IN (100, 200);
SELECT check_all('a two-group UPDATE');
DELETE FROM tb_contract WHERE id_contract = 200 AND version_no = 2;
SELECT check_all('a DELETE of the winning version');
DELETE FROM tb_contract WHERE id_contract = 200;
SELECT check_all('a DELETE of a whole contract');
INSERT INTO tb_contract (id_contract, version_no, status) VALUES (200, 1, 'back');
SELECT check_all('a contract coming back');

-- ── child keyed on its uuid: the winning row's pk_<entity> changes ──────────
CREATE TABLE tb_doc (pk_doc int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL, rev int NOT NULL, body text);
INSERT INTO tb_doc (id, rev, body) VALUES ('00000000-0000-0000-0000-000000000001', 1, 'r1');
CREATE TABLE tb_note (pk_note int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid DEFAULT gen_random_uuid() NOT NULL, fk_doc int, text text);
INSERT INTO tb_note (fk_doc, text) VALUES (1, 'n1'), (2, 'n2');
SELECT pg_tviews_create('tv_doc', $$
  SELECT DISTINCT ON (d.id) d.pk_doc, d.id, jsonb_build_object('body', d.body) AS data
  FROM tb_doc d ORDER BY d.id, d.rev DESC $$);
SELECT pg_tviews_create('tv_note', $$
  SELECT n.pk_note, n.id, n.fk_doc,
         jsonb_build_object('text', n.text, 'doc', vd.data) AS data
  FROM tb_note n LEFT JOIN v_doc vd ON vd.pk_doc = n.fk_doc $$);

UPDATE tb_doc SET body = 'r1-edited';
SELECT assert_fresh('tv_note', 'pk_note', 'an UPDATE of the winning revision');
-- Revision 2 (pk_doc 2) wins: the note on pk_doc 1 loses its doc, the note on
-- pk_doc 2 gains it.
INSERT INTO tb_doc (id, rev, body) VALUES ('00000000-0000-0000-0000-000000000001', 2, 'r2');
SELECT assert_fresh('tv_doc', 'id', 'a new winning revision');
SELECT assert_fresh('tv_note', 'pk_note', 'a new winning revision');
DELETE FROM tb_doc WHERE rev = 2;
SELECT assert_fresh('tv_note', 'pk_note', 'a DELETE of the winning revision');

\echo '#173 propagation from a DISTINCT ON TVIEW: PASS'

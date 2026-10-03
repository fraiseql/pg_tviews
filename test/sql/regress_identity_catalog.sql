-- ADR 0169: every TVIEW records its identity (the column that names its rows) in
-- the catalog and in tviews.registry, and its table's primary key is on that
-- column, whatever its name. Before, a DISTINCT ON key named identifier, fk_* or
-- *_id gave the table no primary key.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_identity_catalog.sql
--
-- expect-output: identity catalog: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_customer (pk_customer int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_contract (pk_contract int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), id_contract int NOT NULL, version_no int NOT NULL);
CREATE TABLE tb_doc (pk_doc int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL, rev int NOT NULL);
CREATE TABLE tb_country (pk_country int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), identifier text NOT NULL, rev int NOT NULL);
CREATE TABLE tb_price (pk_price int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), fk_product int NOT NULL, valid_from date, amount numeric);
CREATE TABLE tb_task (pk_task int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
CREATE TABLE tb_task_archive (pk_task int GENERATED ALWAYS AS IDENTITY (START 1000) PRIMARY KEY,
  id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO tb_customer (name) VALUES ('c1');
INSERT INTO tb_contract (id_contract, version_no) VALUES (100, 1), (100, 2);
INSERT INTO tb_doc (id, rev) VALUES ('00000000-0000-0000-0000-000000000001', 1);
INSERT INTO tb_country (identifier, rev) VALUES ('fr', 1), ('fr', 2), ('de', 1);
INSERT INTO tb_price (fk_product, valid_from, amount) VALUES (1, '2026-01-01', 1), (1, '2026-02-01', 2);

SELECT pg_tviews_create('tv_customer', $$
  SELECT pk_customer, id, jsonb_build_object('name', name) AS data FROM tb_customer $$);
SELECT pg_tviews_create('tv_contract', $$
  SELECT DISTINCT ON (c.id_contract) c.id_contract AS pk_contract, c.id,
         jsonb_build_object('v', c.version_no) AS data
  FROM tb_contract c ORDER BY c.id_contract, c.version_no DESC $$);
SELECT pg_tviews_create('tv_doc', $$
  SELECT DISTINCT ON (d.id) d.pk_doc, d.id, jsonb_build_object('rev', d.rev) AS data
  FROM tb_doc d ORDER BY d.id, d.rev DESC $$);
SELECT pg_tviews_create('tv_country', $$
  SELECT DISTINCT ON (c.identifier) c.pk_country, c.id, c.identifier,
         jsonb_build_object('rev', c.rev) AS data
  FROM tb_country c ORDER BY c.identifier, c.rev DESC $$);
SELECT pg_tviews_create('tv_price', $$
  SELECT DISTINCT ON (p.fk_product) p.pk_price, p.id, p.fk_product,
         jsonb_build_object('amount', p.amount) AS data
  FROM tb_price p ORDER BY p.fk_product, p.valid_from DESC $$);
SELECT pg_tviews_create('tv_task', $$
  SELECT pk_task, id, jsonb_build_object('title', title) AS data FROM tb_task
  UNION ALL
  SELECT pk_task, id, jsonb_build_object('title', title) AS data FROM tb_task_archive $$);

CREATE FUNCTION primary_key(tv regclass) RETURNS text[] LANGUAGE sql AS $$
  SELECT array_agg(a.attname::text ORDER BY a.attnum) FROM pg_index i
  JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY (i.indkey)
  WHERE i.indrelid = tv AND i.indisprimary $$;

DO $$
DECLARE
    expected CONSTANT jsonb := '{
      "customer": ["pk_customer", "pk"], "contract": ["pk_contract", "distinct_on"],
      "doc": ["id", "distinct_on"], "country": ["identifier", "distinct_on"],
      "price": ["fk_product", "distinct_on"], "task": ["pk_task", "pk"]}';
    e text;
    r record;
BEGIN
    FOR e IN SELECT jsonb_object_keys(expected) LOOP
        SELECT g.identity, m.identity->>'kind' AS kind, primary_key(m.table_oid) AS pk INTO r
          FROM tviews.registry g JOIN tviews.pg_tview_meta m USING (entity) WHERE entity = e;
        IF r.identity IS DISTINCT FROM ARRAY[expected->e->>0] THEN
            RAISE EXCEPTION 'identity FAIL: tv_% has identity %, expected {%}', e, r.identity, expected->e->>0;
        END IF;
        IF r.kind IS DISTINCT FROM expected->e->>1 THEN
            RAISE EXCEPTION 'identity FAIL: tv_% has identity kind %, expected %', e, r.kind, expected->e->>1;
        END IF;
        IF r.pk IS DISTINCT FROM ARRAY[expected->e->>0] THEN
            RAISE EXCEPTION 'identity FAIL: the primary key of tv_% is %, expected {%}', e, r.pk, expected->e->>0;
        END IF;
    END LOOP;
END $$;

-- Re-registration rewrites the same identity.
SELECT count(*) FROM pg_tviews_reregister_all();
DO $$ BEGIN
    IF (SELECT identity FROM tviews.registry WHERE entity = 'country') IS DISTINCT FROM '{identifier}' THEN
        RAISE EXCEPTION 'identity FAIL: re-registration changed the identity of tv_country: %',
            (SELECT identity FROM tviews.registry WHERE entity = 'country');
    END IF;
END $$;

\echo 'identity catalog: PASS'

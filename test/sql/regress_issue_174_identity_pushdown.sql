-- #174: a DISTINCT ON TVIEW was refreshed with `WHERE key::text = $1`, which no
-- index serves and which cannot be pushed below the DISTINCT ON: every refresh
-- deduplicated the whole view. The refresh now filters on the identity bound with
-- its type, `key = ANY($1::int8[])` or `= ANY($1::text[]::uuid[])`, which
-- PostgreSQL pushes below the DISTINCT ON into the base table's index.
--
-- The plans below are those of the refresh's source query (refresh/bulk.rs).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_174_identity_pushdown.sql
--
-- expect-output: #174 identity filter reaches the index: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_contract (pk_contract int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid DEFAULT gen_random_uuid() NOT NULL, id_contract int NOT NULL, version_no int NOT NULL, status text);
CREATE INDEX ON tb_contract (id_contract, version_no);
INSERT INTO tb_contract (id_contract, version_no, status)
  SELECT g / 2, g % 2, 's' FROM generate_series(1, 50000) g;
CREATE TABLE tb_order (pk_order bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(), ref text);
CREATE TABLE tb_line (pk_line bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  fk_order bigint NOT NULL, sku text, id uuid NOT NULL DEFAULT gen_random_uuid());
CREATE INDEX ON tb_line (fk_order);
INSERT INTO tb_order (ref) SELECT 'o' || g FROM generate_series(1, 20000) g;
INSERT INTO tb_line (fk_order, sku) SELECT 1 + g % 20000, 's' FROM generate_series(1, 40000) g;
CREATE VIEW v_cnt AS SELECT fk_order, count(*) n FROM tb_line GROUP BY fk_order;
ANALYZE tb_contract, tb_order, tb_line;

SELECT pg_tviews_create('tv_contract', $$
  SELECT DISTINCT ON (c.id_contract) c.id_contract AS pk_contract, c.id,
         jsonb_build_object('status', c.status) AS data
  FROM tb_contract c ORDER BY c.id_contract, c.version_no DESC $$);
-- The #169 shape: keyed on a uuid, tables read through a join.
SELECT pg_tviews_create('tv_order', $$
  SELECT DISTINCT ON (o.id) o.pk_order, o.id, jsonb_build_object('ref', o.ref, 'n', v.n) AS data
  FROM tb_order o LEFT JOIN v_cnt v ON v.fk_order = o.pk_order ORDER BY o.id $$);

CREATE FUNCTION plan_of(q text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE line text; plan text := '';
BEGIN
    FOR line IN EXECUTE 'EXPLAIN (COSTS OFF) ' || q LOOP plan := plan || line || E'\n'; END LOOP;
    RETURN plan;
END $$;

DO $$
DECLARE plan text;
BEGIN
    plan := plan_of($q$SELECT * FROM tviews.public__tv_contract
                       WHERE pk_contract = ANY('{100,200}'::pg_catalog.int8[])$q$);
    IF plan NOT LIKE '%Index%tb_contract%' OR plan LIKE '%Seq Scan on tb_contract%' THEN
        RAISE EXCEPTION '#174 FAIL: the int8 identity filter does not reach the index:%', E'\n' || plan;
    END IF;
    plan := plan_of(format($q$SELECT * FROM tviews.public__tv_order
                              WHERE id = ANY(%L::pg_catalog.text[]::pg_catalog.uuid[])$q$,
                           ARRAY[(SELECT id FROM tb_order WHERE pk_order = 7)::text]));
    IF plan NOT LIKE '%Index%tb_order_id_key%' OR plan LIKE '%Seq Scan on tb_order%' THEN
        RAISE EXCEPTION '#174 FAIL: the uuid identity filter does not reach the index:%', E'\n' || plan;
    END IF;
END $$;

-- And the refreshes they serve are right.
\ir lib/assert_fresh.sql
UPDATE tb_contract SET status = 'x' WHERE id_contract IN (100, 200);
SELECT assert_fresh('tv_contract', 'pk_contract', 'a two-group UPDATE');
INSERT INTO tb_line (fk_order, sku) VALUES (7, 'n');
SELECT assert_fresh('tv_order', 'id', 'an INSERT into tb_line');

\echo '#174 identity filter reaches the index: PASS'

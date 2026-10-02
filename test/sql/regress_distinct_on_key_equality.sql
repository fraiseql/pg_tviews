-- A view column equal to its DISTINCT ON (or GROUP BY) key through a join passes
-- through like the key itself (#162, second ask). `DISTINCT ON (l.fk_order)
-- o.pk_order AS order_pk` with `l.fk_order = o.pk_order`: order_pk is the key's
-- value on every row that can match the TVIEW (NULL on the others), so writes to
-- tb_line map to the orders they belong to instead of being left uncascaded.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_distinct_on_key_equality.sql
--
-- expect-output: DISTINCT ON key equality: PASS
-- reject-output: will not refresh public.tv_order

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_order (pk_order bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                       id uuid NOT NULL DEFAULT gen_random_uuid(), ref text);
CREATE TABLE tb_line (pk_line bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      fk_order bigint NOT NULL REFERENCES tb_order, sku text);
INSERT INTO tb_order (ref) VALUES ('o1'), ('o2');
INSERT INTO tb_line (fk_order, sku) VALUES (1, 'a'), (1, 'b'), (2, 'c');

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_order t FULL JOIN v_order v USING (pk_order)
               WHERE t.pk_order IS NULL OR v.pk_order IS NULL OR t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION 'D1 FAIL: tv_order stale after %', label;
    END IF;
END $$;

CREATE VIEW v_last_line AS
  SELECT DISTINCT ON (l.fk_order) o.pk_order AS order_pk, l.sku
  FROM tb_line l LEFT JOIN tb_order o ON l.fk_order = o.pk_order
  ORDER BY l.fk_order, l.pk_line DESC;
SELECT pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, o.ref, jsonb_build_object('ref', o.ref, 'last', v.sku) AS data
  FROM tb_order o LEFT JOIN v_last_line v ON v.order_pk = o.pk_order $$);

INSERT INTO tb_line (fk_order, sku) VALUES (2, 'd');
SELECT check_fresh('an INSERT into tb_line');
UPDATE tb_line SET sku = 'b2' WHERE sku = 'b';
SELECT check_fresh('an UPDATE of tb_line');
UPDATE tb_line SET fk_order = 2 WHERE sku = 'b2';
SELECT check_fresh('an fk move in tb_line');
DELETE FROM tb_line WHERE sku = 'd';
SELECT check_fresh('a DELETE from tb_line');
UPDATE tb_order SET ref = 'o1-new' WHERE pk_order = 1;
SELECT check_fresh('an UPDATE of tb_order');

DO $$ BEGIN
    IF (SELECT cascade_kinds->>'tb_line' FROM tviews.registry WHERE entity = 'order') = 'all_keys' THEN
        RAISE EXCEPTION 'D1 FAIL: tb_line is still all_keys: %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'order');
    END IF;
END $$;

-- The same with GROUP BY: a column equal to the group key passes through.
DROP TABLE tv_order;
CREATE VIEW v_count AS
  SELECT o.pk_order AS order_pk, count(*) AS n
  FROM tb_line l JOIN tb_order o ON l.fk_order = o.pk_order
  GROUP BY l.fk_order, o.pk_order;
SELECT pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, jsonb_build_object('n', v.n) AS data
  FROM tb_order o LEFT JOIN v_count v ON v.order_pk = o.pk_order $$);
INSERT INTO tb_line (fk_order, sku) VALUES (1, 'e');
SELECT check_fresh('an INSERT into tb_line (GROUP BY)');

\echo 'DISTINCT ON key equality: PASS'

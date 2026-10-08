-- Regression test for issue #165: a table linked to the TVIEW key through the
-- nullable side of an outer join was reported uncascaded. A changed row of the
-- preserved side reaches the key only through a row of the nullable side; one with
-- no match yields NULLs there and matches no key. So the link is followed when the
-- path goes on from the nullable side by an equality.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_outer_join_links.sql
--
-- expect-output: issue #165 outer join links: PASS
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
                      fk_order bigint, sku text);
INSERT INTO tb_order (ref) VALUES ('o1'), ('o2');
INSERT INTO tb_line (fk_order, sku) VALUES (1, 'a'), (1, 'b'), (2, 'c');

CREATE VIEW v_line AS SELECT l.pk_line, l.sku, o.id AS order_id
  FROM tb_line l LEFT JOIN tb_order o ON l.fk_order = o.pk_order;

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_order t FULL JOIN tviews.public__tv_order v USING (pk_order)
               WHERE t.pk_order IS NULL OR v.pk_order IS NULL OR t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION '#165 FAIL: tv_order stale after %', label;
    END IF;
END $$;

SELECT pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id,
         jsonb_build_object('skus', (SELECT jsonb_agg(v.sku ORDER BY v.pk_line)
                                     FROM v_line v WHERE v.order_id = o.id)) AS data
  FROM tb_order o $$);

UPDATE tb_line SET sku = 'b2' WHERE sku = 'b';
SELECT check_fresh('an UPDATE of tb_line');
UPDATE tb_line SET fk_order = 2 WHERE sku = 'a';          -- moves between orders
SELECT check_fresh('an fk move in tb_line');
INSERT INTO tb_line (fk_order, sku) VALUES (99, 'orphan'); -- matches no order
SELECT check_fresh('an INSERT with no matching order');
UPDATE tb_line SET fk_order = 1 WHERE sku = 'orphan';      -- now it matches
SELECT check_fresh('an orphan line attached to an order');
DELETE FROM tb_line WHERE sku = 'c';
SELECT check_fresh('a DELETE from tb_line');

DO $$ BEGIN
    IF (SELECT cascade_kinds->>'tb_line' FROM tviews.registry WHERE entity = 'order') <> 'mapped' THEN
        RAISE EXCEPTION '#165 FAIL: tb_line is %, expected mapped',
            (SELECT cascade_kinds->>'tb_line' FROM tviews.registry WHERE entity = 'order');
    END IF;
END $$;

-- ── keyed on the nullable side itself ───────────────────────────────────────
DROP TABLE tv_order;
SELECT pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, jsonb_build_object('skus', jsonb_agg(l.sku ORDER BY l.pk_line)) AS data
  FROM tb_line l LEFT JOIN tb_order o ON l.fk_order = o.pk_order
  WHERE o.pk_order IS NOT NULL
  GROUP BY o.pk_order, o.id $$);
UPDATE tb_line SET sku = 'b3' WHERE sku = 'b2';
SELECT check_fresh('an UPDATE of tb_line (keyed on the nullable side)');
UPDATE tb_line SET fk_order = 1 WHERE sku = 'a';
SELECT check_fresh('an fk move (keyed on the nullable side)');
INSERT INTO tb_line (fk_order, sku) VALUES (98, 'orphan 2');
SELECT check_fresh('an orphan INSERT (keyed on the nullable side)');

\echo 'issue #165 outer join links: PASS'

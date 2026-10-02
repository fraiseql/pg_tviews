-- Regression test for issue #162: a write to a TVIEW's own table stopped
-- refreshing its row when a second read of the same table (inside a DISTINCT ON
-- view) could not be traced to the key. One untraceable read made the whole table
-- `all_keys`, and under the default `warn` policy nothing refreshed. The traceable
-- reads now keep refreshing; only the rest is left to uncascaded_policy.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_162_partial_mapping.sql
--
-- expect-output: issue #162 partial mapping: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO ERROR;
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

-- ── #162's repro: the own table is also read inside a DISTINCT ON view ───────
CREATE VIEW v_last_line AS
  SELECT DISTINCT ON (l.fk_order) o.pk_order AS order_pk, l.sku
  FROM tb_line l LEFT JOIN tb_order o ON l.fk_order = o.pk_order
  ORDER BY l.fk_order, l.pk_line DESC;
SELECT pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, o.ref, jsonb_build_object('ref', o.ref, 'last', v.sku) AS data
  FROM tb_order o LEFT JOIN v_last_line v ON v.order_pk = o.pk_order $$);

UPDATE tb_order SET ref = 'o1-new' WHERE pk_order = 1;
DO $$ BEGIN
    IF (SELECT ref FROM tv_order WHERE pk_order = 1) IS DISTINCT FROM 'o1-new'
       OR (SELECT data->>'ref' FROM tv_order WHERE pk_order = 1) IS DISTINCT FROM 'o1-new' THEN
        RAISE EXCEPTION '#162 FAIL: an UPDATE of tb_order did not refresh its own row: %',
            (SELECT data FROM tv_order WHERE pk_order = 1);
    END IF;
END $$;
INSERT INTO tb_order (ref) VALUES ('o3');
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM tv_order WHERE ref = 'o3') THEN
        RAISE EXCEPTION '#162 FAIL: an INSERT into tb_order did not add its row';
    END IF;
END $$;

-- The untraceable read is still reported, and still not mapped under warn.
DO $$ BEGIN
    IF (SELECT cascade_kinds->>'tb_order' FROM tviews.registry WHERE entity = 'order') <> 'all_keys'
       OR (SELECT uncascaded_tables::text FROM tviews.registry WHERE entity = 'order') NOT LIKE '%tb_line%' THEN
        RAISE EXCEPTION '#162 FAIL: the untraceable reads are no longer reported: %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'order');
    END IF;
END $$;

-- ── a mapped (two-hop) read plus an untraceable one ─────────────────────────
CREATE TABLE tb_sku (pk_sku bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, code text, label text);
INSERT INTO tb_sku (code, label) VALUES ('a', 'A'), ('b', 'B'), ('c', 'C');
SELECT pg_tviews_create('tv_basket', $$
  SELECT o.pk_order AS pk_basket, o.id, o.pk_order,
         jsonb_build_object(
           'labels', (SELECT jsonb_agg(s.label ORDER BY s.label) FROM tb_line l
                      JOIN tb_sku s ON s.code = l.sku WHERE l.fk_order = o.pk_order),
           'skus', (SELECT count(*) FROM tb_sku)) AS data
  FROM tb_order o $$);
UPDATE tb_sku SET label = 'A2' WHERE code = 'a';
DO $$ BEGIN
    IF (SELECT data->'labels' FROM tv_basket WHERE pk_basket = 1)
       IS DISTINCT FROM (SELECT data->'labels' FROM v_basket WHERE pk_basket = 1) THEN
        RAISE EXCEPTION '#162 FAIL: the mapped read of tb_sku stopped refreshing: % vs %',
            (SELECT data->'labels' FROM tv_basket WHERE pk_basket = 1),
            (SELECT data->'labels' FROM v_basket WHERE pk_basket = 1);
    END IF;
END $$;

-- ── full_refresh still refreshes everything ─────────────────────────────────
DROP TABLE tv_order;
SET pg_tviews.uncascaded_policy = 'full_refresh';
SELECT pg_tviews_create('tv_order', $$
  SELECT o.pk_order, o.id, o.ref, jsonb_build_object('ref', o.ref, 'last', v.sku) AS data
  FROM tb_order o LEFT JOIN v_last_line v ON v.order_pk = o.pk_order $$);
RESET pg_tviews.uncascaded_policy;
INSERT INTO tb_line (fk_order, sku) VALUES (2, 'z');
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tv_order t FULL JOIN v_order v USING (pk_order)
               WHERE t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION '#162 FAIL: full_refresh no longer refreshes on a tb_line write';
    END IF;
END $$;

\echo 'issue #162 partial mapping: PASS'

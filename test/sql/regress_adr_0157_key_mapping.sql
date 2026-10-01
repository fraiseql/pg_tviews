-- ADR 0157, part C: every mapped base table gets a query that turns a set of
-- changed rows (the relation pg_tviews_delta) into the TVIEW keys they can affect.
-- Local tables read the key off the row. The queries resolve nothing through
-- search_path, and one that would scan a large table sequentially is reported
-- at create time with the index to add.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_adr_0157_key_mapping.sql
--
-- expect-output: writes to public.tb_sku map to tv_order keys with a sequential scan of tb_line
-- expect-output: an index on tb_line (fk_sku) would make them cheaper
-- expect-output: ADR 0157 key mapping: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       ref text, min_pos int NOT NULL DEFAULT 0);
CREATE TABLE tb_sku   (pk_sku bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_line  (pk_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_order bigint NOT NULL REFERENCES tb_order, fk_sku bigint REFERENCES tb_sku,
                       pos int NOT NULL, sku text);
CREATE TABLE tb_late  (pk_late bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       min_pos int NOT NULL);
CREATE TABLE tb_invoice (pk_invoice bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid());

INSERT INTO tb_order (pk_order, ref) SELECT g, 'o' || g FROM generate_series(1, 5) g;
INSERT INTO tb_sku (pk_sku, name) SELECT g, 's' || g FROM generate_series(1, 3) g;
-- Enough lines that a sequential scan of tb_line is worth an index.
INSERT INTO tb_line (pk_line, fk_order, fk_sku, pos, sku)
    SELECT g, 1 + g % 5, 1 + g % 3, g % 10, 'x' FROM generate_series(1, 20000) g;
ANALYZE tb_line;
INSERT INTO tb_late (pk_late, min_pos) VALUES (1, 0), (2, 5), (3, 9), (4, 100);
INSERT INTO tb_invoice (pk_invoice) SELECT g FROM generate_series(1, 5) g;
CREATE VIEW v_invoice_lines AS
    SELECT l.fk_order, count(*) AS n FROM tb_line l GROUP BY l.fk_order;

SET client_min_messages TO NOTICE;
-- Two hops: tb_sku -> tb_line -> tb_order (and no index on tb_line.fk_sku).
SELECT pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id,
           jsonb_build_object('ref', o.ref,
               'skus', (SELECT jsonb_agg(DISTINCT s.name) FROM tb_line l JOIN tb_sku s ON s.pk_sku = l.fk_sku
                        WHERE l.fk_order = o.pk_order)) AS data
    FROM tb_order o $$);
SET client_min_messages TO WARNING;
-- A non-equality correlation.
SELECT pg_tviews_create('tv_late', $$
    SELECT t.pk_late, t.id,
           jsonb_build_object('late', EXISTS (SELECT 1 FROM tb_line l WHERE l.pos > t.min_pos)) AS data
    FROM tb_late t $$);
-- #158: a view with GROUP BY, joined on its group key.
SELECT pg_tviews_create('tv_invoice', $$
    SELECT i.pk_invoice, i.id, jsonb_build_object('n', v.n) AS data
    FROM tb_invoice i LEFT JOIN v_invoice_lines v ON v.fk_order = i.pk_invoice $$);

-- The keys the stored mapping of (entity, table) returns for `rows`, rows of the
-- table given as a VALUES list over its columns: the query runs with an empty
-- search_path, over a temporary pg_tviews_delta.
CREATE FUNCTION mapped_keys(e text, tbl text, cols text, rows text) RETURNS bigint[]
LANGUAGE plpgsql AS $$
DECLARE m jsonb; q text; keys bigint[];
BEGIN
    SELECT x INTO m FROM tviews.pg_tview_meta, jsonb_array_elements(key_mappings) x
     WHERE entity = e AND (x->>'relid')::oid = tbl::regclass::oid;
    IF m IS NULL THEN RAISE EXCEPTION 'no mapping of % for %', tbl, e; END IF;
    q := CASE m->>'kind'
           WHEN 'mapped' THEN m->>'sql'
           WHEN 'local' THEN format('SELECT DISTINCT %I FROM pg_tviews_delta', m->>'column')
         END;
    IF q IS NULL THEN RAISE EXCEPTION '% of % is %', tbl, e, m->>'kind'; END IF;
    DROP TABLE IF EXISTS pg_tviews_delta;
    EXECUTE format('CREATE TEMP TABLE pg_tviews_delta AS SELECT * FROM %s LIMIT 0', tbl);
    EXECUTE format('INSERT INTO pg_tviews_delta (%s) VALUES %s', cols, rows);
    PERFORM set_config('search_path', '', true);
    EXECUTE format('SELECT array_agg(k ORDER BY k) FROM (%s) s(k)', q) INTO keys;
    PERFORM set_config('search_path', 'public, tviews', true);
    RETURN keys;
END $$;

CREATE FUNCTION must(got bigint[], want bigint[], what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF got IS DISTINCT FROM want THEN
        RAISE EXCEPTION 'FAIL ADR 0157 key mapping [%]: got %, want %', what, got, want;
    END IF;
END $$;

BEGIN;
-- A changed sku: every order with a line of that sku.
SELECT must(mapped_keys('order', 'tb_sku', 'pk_sku, name', $$(2, 'x')$$),
            ARRAY(SELECT DISTINCT fk_order FROM tb_line WHERE fk_sku = 2 ORDER BY 1), 'two hops');
-- A line moving from order 1 to order 3: OLD and NEW images give both orders.
SELECT must(mapped_keys('order', 'tb_line', 'pk_line, fk_order, fk_sku, pos, sku',
                        $$(7, 1, 1, 1, 'x'), (7, 3, 1, 1, 'x')$$),
            ARRAY[1, 3]::bigint[], 'fk move');
-- l.pos > t.min_pos: a line at pos 6 affects the rows with min_pos below 6.
SELECT must(mapped_keys('late', 'tb_line', 'pk_line, fk_order, fk_sku, pos, sku', $$(8, 1, 1, 6, 'x')$$),
            ARRAY[1, 2]::bigint[], 'non-equality');
-- Through the GROUP BY view: the group key.
SELECT must(mapped_keys('invoice', 'tb_line', 'pk_line, fk_order, fk_sku, pos, sku',
                        $$(9, 4, 1, 1, 'x'), (9, 5, 1, 1, 'x')$$),
            ARRAY[4, 5]::bigint[], 'GROUP BY view');
COMMIT;

-- The stored mapping lists the columns of the table the TVIEW reads.
DO $$ BEGIN
    IF (SELECT x->'columns' FROM tviews.pg_tview_meta, jsonb_array_elements(key_mappings) x
        WHERE entity = 'order' AND (x->>'relid')::oid = 'tb_line'::regclass::oid)
       IS DISTINCT FROM '["fk_order", "fk_sku"]'::jsonb THEN
        RAISE EXCEPTION 'FAIL ADR 0157 key mapping: tb_line columns of tv_order are %',
            (SELECT x->'columns' FROM tviews.pg_tview_meta, jsonb_array_elements(key_mappings) x
             WHERE entity = 'order' AND (x->>'relid')::oid = 'tb_line'::regclass::oid);
    END IF;
END $$;

\echo 'ADR 0157 key mapping: PASS'

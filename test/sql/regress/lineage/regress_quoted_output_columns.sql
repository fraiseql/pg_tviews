-- Regression test (#89): output columns with quoted identifiers.
--
-- `qty AS "order"` used to be recorded as the column name `"order"` (quotes
-- included), so pg_tviews_create() built a column literally named `"order"` and the
-- initial INSERT failed. Unquoted aliases worked. The parser must return the
-- identifier PostgreSQL uses: quotes stripped, `""` unescaped, case kept when quoted,
-- folded to lower case when not. Every refresh path must then quote those names:
-- single-row refresh, bulk refresh, cascade and DISTINCT ON.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_quoted_output_columns.sql
-- expect-output: quoted_output_columns: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tb_item CASCADE;
DROP TABLE IF EXISTS tb_line CASCADE;
DROP TABLE IF EXISTS tb_category CASCADE;
DROP TABLE IF EXISTS tb_doc CASCADE;

-- Rows of `tv` and `v` that differ in either direction, over the given columns.
CREATE FUNCTION divergence(tv text, v text, cols text) RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE d bigint;
BEGIN
    EXECUTE format(
        'SELECT count(*) FROM ((SELECT %1$s FROM %2$s EXCEPT SELECT %1$s FROM %3$s)
                     UNION ALL (SELECT %1$s FROM %3$s EXCEPT SELECT %1$s FROM %2$s)) d',
        cols, tv::regclass, v::regclass) INTO d;
    RETURN d;
END $$;

CREATE FUNCTION assert_consistent(tv text, v text, cols text, step text)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF divergence(tv, v, cols) <> 0 THEN
        RAISE EXCEPTION '#89 FAIL after %: % diverges from %', step, tv, v;
    END IF;
END $$;

-- (1) A standalone TVIEW: single-row refresh is a full-row upsert.
CREATE TABLE tb_item (
    pk_item int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL,
    qty     int  NOT NULL
);
INSERT INTO tb_item (pk_item, name, qty) VALUES (1, 'a', 1), (2, 'b', 2), (3, 'c', 3);

SELECT pg_tviews_create('tv_item', $v$
    SELECT pk_item, id,
           qty  AS "order",
           name AS "Label",
           name AS "with space",
           name AS Folded,
           jsonb_build_object('n', upper(name)) AS data
    FROM tb_item $v$);

-- The TVIEW's columns carry PostgreSQL's identifier values.
DO $$
DECLARE cols text;
BEGIN
    SELECT string_agg(attname, ',' ORDER BY attnum) INTO cols
    FROM pg_attribute
    WHERE attrelid = 'tv_item'::regclass AND attnum > 0 AND NOT attisdropped
      AND attname NOT IN ('created_at', 'updated_at');
    IF cols <> 'pk_item,id,data,order,Label,with space,folded' THEN
        RAISE EXCEPTION '#89 FAIL: unexpected columns: %', cols;
    END IF;
END $$;

\set item_cols 'pk_item, id, "order", "Label", "with space", folded, data'
SELECT assert_consistent('tv_item', 'tviews.public__tv_item', :'item_cols', 'create');

UPDATE tb_item SET qty = 10, name = 'x' WHERE pk_item = 1;
SELECT assert_consistent('tv_item', 'tviews.public__tv_item', :'item_cols', 'single-row UPDATE');
DO $$ BEGIN
    IF (SELECT "order" FROM tv_item WHERE pk_item = 1) <> 10 THEN
        RAISE EXCEPTION '#89 FAIL: single-row refresh did not update "order"';
    END IF;
END $$;

INSERT INTO tb_item (pk_item, name, qty) VALUES (4, 'd', 4);
SELECT assert_consistent('tv_item', 'tviews.public__tv_item', :'item_cols', 'single-row INSERT');

UPDATE tb_item SET qty = qty + 100, name = name || '!';
SELECT assert_consistent('tv_item', 'tviews.public__tv_item', :'item_cols', 'bulk UPDATE');

DELETE FROM tb_item WHERE pk_item = 2;
SELECT assert_consistent('tv_item', 'tviews.public__tv_item', :'item_cols', 'DELETE');

SELECT pg_tviews_drop('tv_item');

-- (2) A TVIEW joined to a parent table: INSERT goes through the smart-patch upsert,
-- and a parent change cascades into it.
CREATE TABLE tb_category (
    pk_category int PRIMARY KEY,
    id          uuid NOT NULL DEFAULT gen_random_uuid(),
    title       text NOT NULL
);
CREATE TABLE tb_line (
    pk_line     int PRIMARY KEY,
    id          uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_category int NOT NULL REFERENCES tb_category (pk_category),
    qty         int NOT NULL
);
INSERT INTO tb_category (pk_category, title) VALUES (1, 'c1'), (2, 'c2');
INSERT INTO tb_line (pk_line, fk_category, qty) VALUES (1, 1, 1), (2, 1, 2), (3, 2, 3);

SELECT pg_tviews_create('tv_line', $v$
    SELECT l.pk_line, l.id, l.fk_category,
           l.qty AS "order",
           l.qty AS "Qty Label",
           jsonb_build_object('qty', l.qty, 'category', c.title) AS data
    FROM tb_line l JOIN tb_category c ON c.pk_category = l.fk_category $v$);

\set line_cols 'pk_line, id, fk_category, "order", "Qty Label", data'
SELECT assert_consistent('tv_line', 'tviews.public__tv_line', :'line_cols', 'create');

INSERT INTO tb_line (pk_line, fk_category, qty) VALUES (4, 2, 4);
SELECT assert_consistent('tv_line', 'tviews.public__tv_line', :'line_cols', 'single-row INSERT');

UPDATE tb_category SET title = 'c1 renamed' WHERE pk_category = 1;
SELECT assert_consistent('tv_line', 'tviews.public__tv_line', :'line_cols', 'cascade UPDATE');

SELECT pg_tviews_drop('tv_line');

-- DISTINCT ON TVIEW with quoted columns: refresh goes through the dedup-key upsert.
CREATE TABLE tb_doc (
    pk_doc      int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id          uuid NOT NULL DEFAULT gen_random_uuid(),
    id_doc      int  NOT NULL,
    version_no  int  NOT NULL,
    status      text
);
INSERT INTO tb_doc (id_doc, version_no, status) VALUES
    (100, 1, 'draft'), (100, 2, 'active'), (200, 1, 'draft');

SELECT pg_tviews_create('tv_doc', $v$
    SELECT DISTINCT ON (v.id_doc)
           v.id_doc     AS pk_doc,
           v.id,
           v.version_no AS "Version",
           v.status     AS "order",
           jsonb_build_object('status', v.status) AS data
    FROM tb_doc v
    ORDER BY v.id_doc, v.version_no DESC $v$);

UPDATE tb_doc SET status = 'signed' WHERE id_doc = 100 AND version_no = 2;
INSERT INTO tb_doc (id_doc, version_no, status) VALUES (200, 2, 'active');

DO $$
DECLARE d bigint;
BEGIN
    SELECT count(*) INTO d FROM (
        (SELECT pk_doc, id, "Version", "order", data FROM tv_doc
         EXCEPT SELECT pk_doc, id, "Version", "order", data FROM tviews.public__tv_doc)
        UNION ALL
        (SELECT pk_doc, id, "Version", "order", data FROM tviews.public__tv_doc
         EXCEPT SELECT pk_doc, id, "Version", "order", data FROM tv_doc)
    ) x;
    IF d <> 0 THEN
        RAISE EXCEPTION '#89 FAIL: tv_doc diverges from tviews.public__tv_doc (% rows)', d;
    END IF;
    IF (SELECT "order" FROM tv_doc WHERE pk_doc = 100) <> 'signed'
       OR (SELECT "Version" FROM tv_doc WHERE pk_doc = 200) <> 2 THEN
        RAISE EXCEPTION '#89 FAIL: DISTINCT ON refresh did not update quoted columns';
    END IF;
END $$;

SELECT pg_tviews_drop('tv_doc');

-- (4) A mixed-case entity: every generated statement must quote the tv_/v_/pk_/fk_
-- names derived from it, for its own refresh and for propagation to a parent.
CREATE TABLE "tb_Mixed" (
    "pk_Mixed" int PRIMARY KEY,
    id         uuid NOT NULL DEFAULT gen_random_uuid(),
    x          text NOT NULL
);
CREATE TABLE tb_holder (
    pk_holder  int PRIMARY KEY,
    id         uuid NOT NULL DEFAULT gen_random_uuid(),
    "fk_Mixed" int NOT NULL REFERENCES "tb_Mixed"
);
INSERT INTO "tb_Mixed" VALUES (1, DEFAULT, 'a'), (2, DEFAULT, 'b');
INSERT INTO tb_holder VALUES (1, DEFAULT, 1), (2, DEFAULT, 1), (3, DEFAULT, 2);

SELECT pg_tviews_create('tv_Mixed', $v$
    SELECT "pk_Mixed", id, jsonb_build_object('x', x) AS data FROM "tb_Mixed" $v$);
SELECT pg_tviews_create('tv_holder', $v$
    SELECT h.pk_holder, h.id, h."fk_Mixed",
           jsonb_build_object('mixed', m.data) AS data
    FROM tb_holder h JOIN "tv_Mixed" m ON m."pk_Mixed" = h."fk_Mixed" $v$);

\set mixed_cols '"pk_Mixed", id, data'
\set holder_cols 'pk_holder, id, "fk_Mixed", data'

INSERT INTO "tb_Mixed" VALUES (3, DEFAULT, 'c');
SELECT assert_consistent('"tv_Mixed"', 'tviews."public__tv_Mixed"', :'mixed_cols', 'mixed-case INSERT');

UPDATE "tb_Mixed" SET x = 'a2' WHERE "pk_Mixed" = 1;
SELECT assert_consistent('"tv_Mixed"', 'tviews."public__tv_Mixed"', :'mixed_cols', 'mixed-case single-row UPDATE');
SELECT assert_consistent('tv_holder', 'tviews.public__tv_holder', :'holder_cols', 'propagation from mixed-case entity');

UPDATE "tb_Mixed" SET x = x || '!';
SELECT assert_consistent('"tv_Mixed"', 'tviews."public__tv_Mixed"', :'mixed_cols', 'mixed-case bulk UPDATE');
SELECT assert_consistent('tv_holder', 'tviews.public__tv_holder', :'holder_cols', 'bulk propagation from mixed-case entity');

DELETE FROM "tb_Mixed" WHERE "pk_Mixed" = 3;
SELECT assert_consistent('"tv_Mixed"', 'tviews."public__tv_Mixed"', :'mixed_cols', 'mixed-case DELETE');

DO $$ BEGIN
    IF (SELECT data->'mixed'->>'x' FROM tv_holder WHERE pk_holder = 1) <> 'a2!' THEN
        RAISE EXCEPTION '#89 FAIL: tv_holder did not see the mixed-case child change';
    END IF;
END $$;

SELECT pg_tviews_drop('tv_holder');
SELECT pg_tviews_drop('tv_Mixed');

\echo 'quoted_output_columns: PASS'

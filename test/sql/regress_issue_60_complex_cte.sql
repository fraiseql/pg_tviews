-- Regression test for issue #60: cascade tracking through complex CTEs.
--
-- Cascade paths resolved only a CTE whose body was one SELECT over one base table.
-- A base table reachable only through a CTE chain, a multi-table CTE body or a
-- UNION-bodied CTE got no cascade path, so changes to it never reached the TVIEW.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_60_complex_cte.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_locale (
    pk_locale BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    code      TEXT NOT NULL
);
CREATE TABLE tb_item (
    pk_item BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    sku     TEXT
);
CREATE TABLE tb_item_i18n (
    pk_item_i18n BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    item_id      BIGINT NOT NULL REFERENCES tb_item(pk_item),
    fk_locale    BIGINT NOT NULL REFERENCES tb_locale(pk_locale),
    label        TEXT
);
CREATE TABLE tb_item_note (
    pk_item_note BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    item_id      BIGINT NOT NULL REFERENCES tb_item(pk_item),
    note         TEXT
);
CREATE TABLE tb_other (pk_other BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, x INT);

INSERT INTO tb_locale (code) VALUES ('fr');
INSERT INTO tb_item (sku) VALUES ('a'), ('b');
INSERT INTO tb_item_i18n (item_id, fk_locale, label) VALUES (1, 1, 'un'), (2, 1, 'deux');
INSERT INTO tb_item_note (item_id, note) VALUES (1, 'n1'), (2, 'n2');
INSERT INTO tb_other (x) VALUES (1);

CREATE FUNCTION must(ok BOOLEAN, msg TEXT) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION '#60 FAIL: %', msg; END IF; END $$;

-- ========================================================================
-- Cycle 1: CTE-on-CTE chain
-- ========================================================================
CREATE TABLE tv_item AS
WITH fr AS (SELECT item_id, label FROM tb_item_i18n WHERE fk_locale = 1),
     up AS (SELECT item_id, upper(label) AS label FROM fr)
SELECT i.pk_item, i.id, jsonb_build_object('sku', i.sku, 'label', up.label) AS data
FROM tb_item i LEFT JOIN up ON up.item_id = i.pk_item;

UPDATE tb_item_i18n SET label = 'uno' WHERE item_id = 1;
SELECT must((SELECT data->>'label' FROM tv_item WHERE pk_item = 1) = 'UNO',
            'a change reachable only through a CTE chain did not refresh tv_item');

-- ========================================================================
-- Cycle 2: multi-table CTE body
-- ========================================================================
CREATE TABLE tb_product (
    pk_product BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_item    BIGINT NOT NULL REFERENCES tb_item(pk_item)
);
INSERT INTO tb_product (fk_item) VALUES (1), (2);
CREATE TABLE tv_product AS
WITH labelled AS (SELECT t.item_id, t.label, loc.code
                  FROM tb_item_i18n t JOIN tb_locale loc ON loc.pk_locale = t.fk_locale)
SELECT p.pk_product, p.id,
       jsonb_build_object('label', labelled.label, 'locale', labelled.code) AS data
FROM tb_product p JOIN labelled ON labelled.item_id = p.fk_item;

UPDATE tb_locale SET code = 'fr-FR' WHERE pk_locale = 1;
SELECT must((SELECT count(*) FROM tv_product WHERE data->>'locale' = 'fr-FR') = 2,
            'a change to a table joined inside a CTE body did not refresh tv_product');
UPDATE tb_item_i18n SET label = 'dos' WHERE item_id = 2;
SELECT must((SELECT data->>'label' FROM tv_product WHERE pk_product = 2) = 'dos',
            'a change to the CTE body''s other table did not refresh tv_product');

-- ========================================================================
-- Cycle 3: UNION-bodied CTE
-- ========================================================================
CREATE TABLE tb_sheet (
    pk_sheet BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id       UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_item  BIGINT NOT NULL REFERENCES tb_item(pk_item)
);
INSERT INTO tb_sheet (fk_item) VALUES (1), (2);
CREATE TABLE tv_sheet AS
WITH texts AS (SELECT item_id, label AS txt FROM tb_item_i18n
               UNION ALL
               SELECT item_id, note FROM tb_item_note)
SELECT s.pk_sheet, s.id,
       jsonb_build_object('texts', (SELECT jsonb_agg(t.txt ORDER BY t.txt) FROM texts t
                                    WHERE t.item_id = s.fk_item)) AS data
FROM tb_sheet s JOIN texts ON texts.item_id = s.fk_item
GROUP BY s.pk_sheet, s.id, s.fk_item;

UPDATE tb_item_note SET note = 'note-1' WHERE item_id = 1;
SELECT must((SELECT data->'texts' ? 'note-1' FROM tv_sheet WHERE pk_sheet = 1),
            'a change to the second UNION branch did not refresh tv_sheet');
UPDATE tb_item_i18n SET label = 'one' WHERE item_id = 1;
SELECT must((SELECT data->'texts' ? 'one' FROM tv_sheet WHERE pk_sheet = 1),
            'a change to the first UNION branch did not refresh tv_sheet');

-- ========================================================================
-- Cycle 4: every TVIEW matches its view; an unrelated table changes nothing
-- ========================================================================
SELECT (pg_tviews_queue_stats()->>'view_recomputes')::bigint AS r0 \gset
UPDATE tb_other SET x = x + 1;
SELECT must((pg_tviews_queue_stats()->>'view_recomputes')::bigint = :r0,
            'an unrelated table change recomputed TVIEW rows');
SELECT must((SELECT count(*) FROM tviews.public__tv_item v FULL JOIN tv_item t USING (pk_item)
             WHERE t.data IS DISTINCT FROM v.data) = 0, 'tv_item out of sync');
SELECT must((SELECT count(*) FROM tviews.public__tv_product v FULL JOIN tv_product t USING (pk_product)
             WHERE t.data IS DISTINCT FROM v.data) = 0, 'tv_product out of sync');
SELECT must((SELECT count(*) FROM tviews.public__tv_sheet v FULL JOIN tv_sheet t USING (pk_sheet)
             WHERE t.data IS DISTINCT FROM v.data) = 0, 'tv_sheet out of sync');

-- Regression test (#98): single-row refresh must update every projected column.
--
-- For a TVIEW with scalar dependencies (it joins a parent table), a single-row
-- change went through the smart-patch upsert, whose DO UPDATE only wrote `data`.
-- Other projected columns (`qty AS qty_alias`, `name AS label`) kept their old
-- values. The direct-patch fast path must not skip them either.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_98_stale_projected_columns.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tb_item CASCADE;
DROP TABLE IF EXISTS tb_note CASCADE;
DROP TABLE IF EXISTS tb_category CASCADE;

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
        RAISE EXCEPTION '#98 FAIL after %: % diverges from %', step, tv, v;
    END IF;
END $$;

-- (1) Joined TVIEW: single-row refresh goes through the smart-patch upsert.
CREATE TABLE tb_category (
    pk_category int PRIMARY KEY,
    id          uuid NOT NULL DEFAULT gen_random_uuid(),
    title       text NOT NULL
);
CREATE TABLE tb_item (
    pk_item     int PRIMARY KEY,
    id          uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_category int NOT NULL REFERENCES tb_category (pk_category),
    name        text NOT NULL,
    qty         int  NOT NULL
);
INSERT INTO tb_category VALUES (1, DEFAULT, 'c1');
INSERT INTO tb_item VALUES (1, DEFAULT, 1, 'a', 1), (2, DEFAULT, 1, 'b', 2);

SELECT pg_tviews_create('tv_item', $v$
    SELECT i.pk_item, i.id, i.fk_category, i.qty AS qty_alias, i.name AS label,
           jsonb_build_object('n', i.name, 'category', c.title) AS data
    FROM tb_item i JOIN tb_category c ON c.pk_category = i.fk_category $v$);

\set item_cols 'pk_item, id, fk_category, qty_alias, label, data'

UPDATE tb_item SET qty = 10, name = 'A' WHERE pk_item = 1;
SELECT assert_consistent('tv_item', 'tviews.public__tv_item', :'item_cols', 'UPDATE of data and extra columns');

UPDATE tb_item SET qty = 20 WHERE pk_item = 2;
SELECT assert_consistent('tv_item', 'tviews.public__tv_item', :'item_cols', 'UPDATE of an extra column only');

UPDATE tb_item SET name = 'B' WHERE pk_item = 2;
SELECT assert_consistent('tv_item', 'tviews.public__tv_item', :'item_cols', 'UPDATE of a directly mapped column');

-- (2) Standalone TVIEW whose extra column shares a directly mapped base column:
-- a direct patch of `data` alone would leave `label` stale.
CREATE TABLE tb_note (
    pk_note int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    body    text NOT NULL
);
INSERT INTO tb_note VALUES (1, DEFAULT, 'x');

SELECT pg_tviews_create('tv_note', $v$
    SELECT pk_note, id, body AS label, jsonb_build_object('body', body) AS data
    FROM tb_note $v$);

UPDATE tb_note SET body = 'y' WHERE pk_note = 1;
SELECT assert_consistent('tv_note', 'tviews.public__tv_note', 'pk_note, id, label, data', 'direct-patch-shaped UPDATE');

SELECT pg_tviews_drop('tv_item');
SELECT pg_tviews_drop('tv_note');

\echo '#98 PASS: single-row refresh keeps every projected column in sync'

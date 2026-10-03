-- #169: `DISTINCT ON (o.id)` on a unique root column was refused as soon as any
-- other table the TVIEW reads had a column named `id`: the key was compared by
-- name, qualifiers stripped. A DISTINCT ON TVIEW is keyed on its DISTINCT ON key,
-- resolved from the query tree, so the tables it reads through joins map to it
-- whatever their column names.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_169_distinct_on_qualified_key.sql
--
-- known-failing: #169
-- expect-output: #169 DISTINCT ON a qualified key: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE TABLE tb_order (pk_order bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                       id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(), ref text);
CREATE TABLE tb_line (pk_line bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      fk_order bigint NOT NULL REFERENCES tb_order, sku text,
                      id uuid NOT NULL DEFAULT gen_random_uuid());
INSERT INTO tb_order (ref) VALUES ('o1'), ('o2');
INSERT INTO tb_line (fk_order, sku) VALUES (1, 'a'), (2, 'b');
CREATE VIEW v_cnt AS SELECT fk_order, count(*) n FROM tb_line GROUP BY fk_order;

-- ── the issue: another table read has an `id` column ────────────────────────
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_order', $q$
      SELECT DISTINCT ON (o.id) o.pk_order, o.id, jsonb_build_object('ref', o.ref, 'n', v.n) AS data
      FROM tb_order o LEFT JOIN v_cnt v ON v.fk_order = o.pk_order ORDER BY o.id $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#169 FAIL: DISTINCT ON (o.id) is refused: %', SQLERRM;
END $$;
INSERT INTO tb_line (fk_order, sku) VALUES (1, 'c');
SELECT assert_fresh('tv_order', 'id', 'an INSERT into tb_line');
UPDATE tb_line SET fk_order = 2 WHERE sku = 'a';
SELECT assert_fresh('tv_order', 'id', 'a line moving to another order');
DELETE FROM tb_line WHERE sku = 'b';
SELECT assert_fresh('tv_order', 'id', 'a DELETE from tb_line');
UPDATE tb_order SET ref = 'x';
SELECT assert_fresh('tv_order', 'id', 'a two-row UPDATE of tb_order');
INSERT INTO tb_order (ref) VALUES ('o3');
SELECT assert_fresh('tv_order', 'id', 'an INSERT into tb_order');
SELECT pg_tviews_drop('tv_order', false, false);

-- ── the key read through a view ─────────────────────────────────────────────
CREATE VIEW v_ord AS SELECT id, ref FROM tb_order;
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_order', $q$
      SELECT DISTINCT ON (vo.id) o.pk_order, vo.id,
             jsonb_build_object('ref', vo.ref, 'n', v.n) AS data
      FROM v_ord vo JOIN tb_order o ON o.id = vo.id
      LEFT JOIN v_cnt v ON v.fk_order = o.pk_order ORDER BY vo.id $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#169 FAIL: DISTINCT ON (vo.id) through a view is refused: %', SQLERRM;
END $$;
INSERT INTO tb_line (fk_order, sku) VALUES (3, 'd');
SELECT assert_fresh('tv_order', 'id', 'an INSERT into tb_line (key through a view)');
UPDATE tb_order SET ref = 'y' WHERE pk_order = 3;
SELECT assert_fresh('tv_order', 'id', 'an UPDATE of tb_order (key through a view)');
SELECT pg_tviews_drop('tv_order', false, false);

-- ── guard: the key is a joined table's column, equal to a projected one ─────
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_order', $q$
      SELECT DISTINCT ON (l.fk_order) o.pk_order, o.id,
             jsonb_build_object('ref', o.ref, 'last', l.sku) AS data
      FROM tb_line l JOIN tb_order o ON o.pk_order = l.fk_order
      ORDER BY l.fk_order, l.pk_line DESC $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#169 FAIL: DISTINCT ON a joined column equal to pk_order is refused: %', SQLERRM;
END $$;
INSERT INTO tb_line (fk_order, sku) VALUES (1, 'e'), (2, 'f');
SELECT assert_fresh('tv_order', 'pk_order', 'new last lines');
UPDATE tb_order SET ref = 'z' WHERE pk_order = 1;
SELECT assert_fresh('tv_order', 'pk_order', 'an UPDATE of tb_order (joined key)');
DELETE FROM tb_line WHERE sku IN ('e', 'f');
SELECT assert_fresh('tv_order', 'pk_order', 'last lines deleted');
SELECT pg_tviews_drop('tv_order', false, false);

-- ── guard: an expression key that is not projected is refused, by name ──────
DO $$
DECLARE msg text;
BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_create('tv_order', $q$
          SELECT DISTINCT ON (lower(o.ref)) o.pk_order, o.id, jsonb_build_object('ref', o.ref) AS data
          FROM tb_order o ORDER BY lower(o.ref), o.pk_order $q$);
        RAISE EXCEPTION '#169 FAIL: an unprojected expression key was accepted';
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT;
    END;
    IF msg LIKE '#169 FAIL%' THEN RAISE EXCEPTION '%', msg; END IF;
    IF msg NOT LIKE '%lower%' THEN
        RAISE EXCEPTION '#169 FAIL: the refusal does not name the key: %', msg;
    END IF;
END $$;

\echo '#169 DISTINCT ON a qualified key: PASS'

-- Regression test for issue #188: a TVIEW over a UNION of two entities with their
-- own key spaces. Each branch derives the TVIEW key from its own table, as a
-- column or an expression of that table's row (a sign, an offset), in the
-- definition or in a view it reads: a write to either table refreshes the keys of
-- its own branch, and a table joined to the union's output refreshes both.
-- Overlapping keys are refused loudly. A key no branch table carries (computed
-- from two tables) is not traceable.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_union_keys.sql
--
-- expect-output: issue #188 union keys: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#188 FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'created';
EXCEPTION WHEN OTHERS THEN RETURN SQLERRM; END $$;
CREATE FUNCTION kinds() RETURNS text LANGUAGE sql AS $$
    SELECT cascade_kinds::text || ' uncascaded=' || uncascaded_tables::text
    FROM tviews.registry WHERE entity = 'attachment' $$;

CREATE TABLE tb_product (pk_product bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         name text, deleted_at timestamptz);
CREATE TABLE tb_order_line (pk_order_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            fk_product bigint REFERENCES tb_product, qty int, deleted_at timestamptz);
CREATE TABLE tb_note (pk_note bigint PRIMARY KEY, target uuid, body text);
INSERT INTO tb_product (pk_product, name) VALUES (1, 'p1'), (2, 'p2');
INSERT INTO tb_order_line (pk_order_line, fk_product, qty) VALUES (1, 1, 5);   -- pk 1 in both tables
INSERT INTO tb_note VALUES (1, (SELECT id FROM tb_product WHERE pk_product = 1), 'on p1'),
                           (2, (SELECT id FROM tb_order_line WHERE pk_order_line = 1), 'on line 1');

-- Writes to every table, each followed by a freshness check.
CREATE FUNCTION exercise(tag text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_product SET name = 'p1x' WHERE pk_product = 1;            -- both branches
    PERFORM assert_fresh('tv_attachment', 'pk_attachment', tag || ': product renamed');
    INSERT INTO tb_order_line (pk_order_line, fk_product, qty) VALUES (2, 2, 1), (3, 1, 7);
    PERFORM assert_fresh('tv_attachment', 'pk_attachment', tag || ': lines inserted');
    UPDATE tb_order_line SET qty = qty + 1, fk_product = 2 WHERE pk_order_line = 1;
    PERFORM assert_fresh('tv_attachment', 'pk_attachment', tag || ': a line updated');
    UPDATE tb_order_line SET deleted_at = now() WHERE pk_order_line = 3;
    PERFORM assert_fresh('tv_attachment', 'pk_attachment', tag || ': a line soft-deleted');
    UPDATE tb_product SET deleted_at = now() WHERE pk_product = 2;      -- drops lines 1 and 2 too
    PERFORM assert_fresh('tv_attachment', 'pk_attachment', tag || ': a product soft-deleted');
    DELETE FROM tb_order_line WHERE pk_order_line = 2;
    PERFORM assert_fresh('tv_attachment', 'pk_attachment', tag || ': a line deleted');
    -- Back to the start.
    UPDATE tb_product SET deleted_at = NULL, name = 'p' || pk_product;
    DELETE FROM tb_order_line WHERE pk_order_line <> 1;
    UPDATE tb_order_line SET qty = 5, fk_product = 1, deleted_at = NULL;
    PERFORM assert_fresh('tv_attachment', 'pk_attachment', tag || ': reset');
END $$;

-- 1. A top-level UNION ALL, the line branch keyed by its negated pk.
\set top 'SELECT p.pk_product AS pk_attachment, p.id, ''catalog'' AS kind, p.name, NULL::int AS qty FROM tb_product p WHERE p.deleted_at IS NULL UNION ALL SELECT -l.pk_order_line, l.id, ''line'', p.name, l.qty FROM tb_order_line l JOIN tb_product p ON p.pk_product = l.fk_product WHERE l.deleted_at IS NULL AND p.deleted_at IS NULL'
SELECT tviews.pg_tviews_create('tv_attachment', :'top');
SELECT must((SELECT count(*) FROM tv_attachment) = 3, 'rows: ' || (SELECT count(*) FROM tv_attachment));
SELECT must((SELECT uncascaded_tables = '{}' AND cascade_kinds ->> 'tb_order_line' = 'mapped'
             FROM tviews.registry WHERE entity = 'attachment'), 'top-level union: ' || kinds());
SELECT exercise('top-level union');
SELECT tviews.pg_tviews_drop('tv_attachment');

-- 2. The same UNION in a view the definition reads, keys inside the branches.
CREATE VIEW v_attachment AS
SELECT p.pk_product AS pk_attachment, p.id, 'catalog' AS kind, p.name, NULL::int AS qty
FROM tb_product p WHERE p.deleted_at IS NULL
UNION ALL
SELECT l.pk_order_line + 1000000000, l.id, 'line', p.name, l.qty
FROM tb_order_line l JOIN tb_product p ON p.pk_product = l.fk_product
WHERE l.deleted_at IS NULL AND p.deleted_at IS NULL;
SELECT tviews.pg_tviews_create('tv_attachment',
    'SELECT v.pk_attachment, v.id, v.kind, v.name, v.qty FROM v_attachment v');
SELECT must((SELECT count(*) FROM tv_attachment) = 3, 'rows through a view');
SELECT must((SELECT uncascaded_tables = '{}' FROM tviews.registry WHERE entity = 'attachment'),
            'union through a view: ' || kinds());
SELECT exercise('union through a view');
SELECT tviews.pg_tviews_drop('tv_attachment');

-- 3. A table joined to the union's output reaches the rows of both branches.
SELECT tviews.pg_tviews_create('tv_attachment', $$
    SELECT v.pk_attachment, v.id, v.kind, v.name, v.qty, n.body AS note
    FROM v_attachment v LEFT JOIN tb_note n ON n.target = v.id $$);
SELECT must((SELECT uncascaded_tables = '{}' AND cascade_kinds ->> 'tb_note' = 'mapped'
             FROM tviews.registry WHERE entity = 'attachment'), 'joined outside: ' || kinds());
UPDATE tb_note SET body = body || '!';
SELECT assert_fresh('tv_attachment', 'pk_attachment', 'notes on both branches updated');
UPDATE tb_note SET target = (SELECT id FROM tb_product WHERE pk_product = 2) WHERE pk_note = 2;
SELECT assert_fresh('tv_attachment', 'pk_attachment', 'a note moved from a line to a product');
SELECT exercise('joined outside');
SELECT tviews.pg_tviews_drop('tv_attachment');

-- 4. A key computed from two tables is no branch table's: refused (the issue's
--    case 2 spelling).
SELECT must(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_attachment', $$
    SELECT COALESCE(p.pk_product, -l.pk_order_line) AS pk_attachment, v.id, v.kind, v.name, v.qty
    FROM (SELECT p.id, 'catalog' AS kind, p.name, NULL::int AS qty FROM tb_product p
          UNION ALL SELECT l.id, 'line', p.name, l.qty FROM tb_order_line l
          JOIN tb_product p ON p.pk_product = l.fk_product) v
    LEFT JOIN tb_product p ON p.id = v.id LEFT JOIN tb_order_line l ON l.id = v.id $$)) <> 'created',
            'a key computed from two tables was accepted');

-- 5. Overlapping keys are loud: at create, and at refresh.
SELECT must(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_attachment',
    replace(:'top', '-l.pk_order_line', 'l.pk_order_line'))) LIKE '%duplicate key%',
            'overlapping keys were accepted at create');
SELECT tviews.pg_tviews_create('tv_attachment', :'top');
INSERT INTO tb_order_line (pk_order_line, fk_product, qty) VALUES (9, 1, 1);
SELECT must(error_of('UPDATE tb_order_line SET pk_order_line = -2 WHERE pk_order_line = 9')
            LIKE '%multiple rows%', 'a key collision at refresh was not reported');
DELETE FROM tb_order_line WHERE pk_order_line = 9;
SELECT assert_fresh('tv_attachment', 'pk_attachment', 'after the refused collision');
SELECT tviews.pg_tviews_drop('tv_attachment');

-- 6. A create that fails leaves nothing behind for the next one in the session:
--    the columns of the refused definition never reach the next TVIEW.
SELECT must(error_of(format('SELECT tviews.pg_tviews_create(%L, %L)', 'tv_attachment',
    'SELECT v.pk_attachment, v.id, v.kind, v.name, v.qty, 1 / 0 AS boom FROM v_attachment v'))
            LIKE '%division by zero%', 'the failing create did not fail');
SELECT tviews.pg_tviews_create('tv_attachment',
    'SELECT v.pk_attachment, v.id, v.kind, v.name, v.qty FROM v_attachment v');
SELECT must((SELECT count(*) FROM tv_attachment) = 3, 'a create after a failed one');

SELECT 'issue #188 union keys: PASS' AS result;

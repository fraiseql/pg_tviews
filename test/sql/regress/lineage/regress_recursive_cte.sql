-- Regression test for issue #183: a backing view that reads a view with WITH
-- RECURSIVE was refused as "more than 32 levels deep" (the walker followed the
-- recursive CTE's reference to itself). A recursive CTE is walked once: the
-- tables read inside it are all_keys ("read in a recursive CTE"), and the tables
-- read outside it keep their mapping. The same holds for recursion written in the
-- definition itself; a definition nesting 33 levels is still refused.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_recursive_cte.sql
--
-- expect-output: issue #183 recursive CTEs: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

-- The lookup tree is small and rarely written: it is refreshed in full.
SET pg_tviews.uncascaded_policy = 'full_refresh';

CREATE TABLE tb_category (pk_category bigint PRIMARY KEY, id uuid UNIQUE NOT NULL DEFAULT gen_random_uuid(),
                          fk_parent bigint REFERENCES tb_category, name text);
CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid UNIQUE NOT NULL DEFAULT gen_random_uuid(),
                      fk_category bigint REFERENCES tb_category, name text, deleted_at timestamptz);
INSERT INTO tb_category VALUES (1, DEFAULT, NULL, 'root'), (2, DEFAULT, 1, 'sub'), (3, DEFAULT, 2, 'leaf');
INSERT INTO tb_item VALUES (1, DEFAULT, 2, 'thing', NULL), (2, DEFAULT, 3, 'other', NULL);

CREATE VIEW v_category_path AS
WITH RECURSIVE p AS (
  SELECT pk_category, ARRAY[name] AS names FROM tb_category WHERE fk_parent IS NULL
  UNION ALL
  SELECT c.pk_category, p.names || c.name FROM tb_category c JOIN p ON p.pk_category = c.fk_parent)
SELECT pk_category, names FROM p;

-- ── the issue's shape: the recursive view read by the item view ─────────────
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_item', $q$
        SELECT i.pk_item, i.id, jsonb_build_object('name', i.name, 'category_path', cp.names) AS data
        FROM tb_item i JOIN v_category_path cp ON cp.pk_category = i.fk_category
        WHERE i.deleted_at IS NULL $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#183 FAIL: a view reading a recursive view is refused: %', SQLERRM;
END $$;
UPDATE tb_item SET name = 'renamed' WHERE pk_item = 1;
SELECT assert_fresh('tv_item', 'pk_item', 'an item rename');
UPDATE tb_item SET deleted_at = now() WHERE pk_item = 2;
SELECT assert_fresh('tv_item', 'pk_item', 'an item soft delete');
UPDATE tb_category SET name = 'top' WHERE pk_category = 1;
SELECT assert_fresh('tv_item', 'pk_item', 'a category rename (full refresh)');
DO $$
DECLARE m jsonb := (SELECT plan->'tables' FROM tviews.pg_tview_meta WHERE entity = 'item');
BEGIN
    IF (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'item')
       <> '{"tb_item": "local", "tb_category": "all_keys"}' THEN
        RAISE EXCEPTION '#183 FAIL: tv_item classifies %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'item');
    END IF;
    IF NOT EXISTS (SELECT 1 FROM jsonb_array_elements(m) e
                   WHERE e->>'table' = 'public.tb_category'
                     AND e->>'reason' LIKE 'read in a recursive CTE (public.v_category_path)%') THEN
        RAISE EXCEPTION '#183 FAIL: the reason for tb_category is %', m;
    END IF;
END $$;

-- ── recursion written in the definition: the same rule ─────────────────────
CREATE TABLE tb_listing (pk_listing bigint PRIMARY KEY, id uuid UNIQUE NOT NULL DEFAULT gen_random_uuid(),
                         fk_category bigint REFERENCES tb_category, name text);
INSERT INTO tb_listing VALUES (1, DEFAULT, 2, 'thing'), (2, DEFAULT, 3, 'other');
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_listing', $q$
        WITH RECURSIVE p AS (
          SELECT pk_category, ARRAY[name] AS names FROM tb_category WHERE fk_parent IS NULL
          UNION ALL
          SELECT c.pk_category, p.names || c.name FROM tb_category c JOIN p ON p.pk_category = c.fk_parent)
        SELECT l.pk_listing, l.id, jsonb_build_object('name', l.name, 'path', p.names) AS data
        FROM tb_listing l JOIN p ON p.pk_category = l.fk_category $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#183 FAIL: WITH RECURSIVE in the definition is refused: %', SQLERRM;
END $$;
UPDATE tb_listing SET fk_category = 3 WHERE pk_listing = 1;
SELECT assert_fresh('tv_listing', 'pk_listing', 'a listing moved to another category');
INSERT INTO tb_category VALUES (4, DEFAULT, 3, 'deeper');
UPDATE tb_listing SET fk_category = 4 WHERE pk_listing = 1;
SELECT assert_fresh('tv_listing', 'pk_listing', 'a new category and a listing moved into it');
DO $$
DECLARE m jsonb := (SELECT plan->'tables' FROM tviews.pg_tview_meta WHERE entity = 'listing');
BEGIN
    IF (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'listing')
       <> '{"tb_listing": "local", "tb_category": "all_keys"}' THEN
        RAISE EXCEPTION '#183 FAIL: tv_listing classifies %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'listing');
    END IF;
    IF NOT EXISTS (SELECT 1 FROM jsonb_array_elements(m) e
                   WHERE e->>'table' = 'public.tb_category'
                     AND e->>'reason' LIKE 'read in a recursive CTE (the definition)%') THEN
        RAISE EXCEPTION '#183 FAIL: the reason for tb_category in tv_listing is %', m;
    END IF;
END $$;

-- ── the key comes out of the recursion: no root, like an opaque top level ───
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_category', $q$
        WITH RECURSIVE p AS (
          SELECT pk_category, id, ARRAY[name] AS names FROM tb_category WHERE fk_parent IS NULL
          UNION ALL
          SELECT c.pk_category, c.id, p.names || c.name FROM tb_category c JOIN p ON p.pk_category = c.fk_parent)
        SELECT pk_category, id, jsonb_build_object('names', names) AS data FROM p $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#183 FAIL: a TVIEW keyed out of a recursive CTE is refused: %', SQLERRM;
END $$;
UPDATE tb_category SET name = 'sub2' WHERE pk_category = 2;
SELECT assert_fresh('tv_category', 'pk_category', 'a category rename');
INSERT INTO tb_category VALUES (5, DEFAULT, 1, 'sibling');
SELECT assert_fresh('tv_category', 'pk_category', 'a category insert');
DO $$ BEGIN
    IF (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'category') <> '{"tb_category": "all_keys"}' THEN
        RAISE EXCEPTION '#183 FAIL: tv_category classifies %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'category');
    END IF;
END $$;

-- ── a definition nesting more than 32 levels is still refused ───────────────
DO $$
DECLARE q text := 'SELECT pk_item, id, name FROM tb_item';
BEGIN
    FOR i IN 1..33 LOOP
        q := format('SELECT pk_item, id, name FROM (%s) s%s', q, i);
    END LOOP;
    PERFORM tviews.pg_tviews_create('tv_deep', format(
        'SELECT pk_item AS pk_deep, id, jsonb_build_object(''name'', name) AS data FROM (%s) s', q));
    RAISE EXCEPTION '#183 FAIL: a definition 35 levels deep was accepted';
EXCEPTION WHEN OTHERS THEN
    IF SQLERRM NOT LIKE '%more than 32 levels deep%' THEN
        RAISE EXCEPTION '#183 FAIL: the 35-level definition is refused for another reason: %', SQLERRM;
    END IF;
END $$;

\echo 'issue #183 recursive CTEs: PASS'

-- Characterization (#238): the fan-out UPDATE and its no-op guard.
--
-- A write to a column that child TVIEWs copy (tb_cat.name into every tv_item
-- row of the category) patches the children in place: one UPDATE setting
-- `data = jsonb_set(t.data, path, value)`, guarded by
-- `t.data IS DISTINCT FROM jsonb_set(t.data, path, value)` so that rows already
-- holding the value are neither written nor journaled. The guard builds the new
-- document a second time; comparing the values at the paths instead was
-- measured (#238) and saves nothing on TOASTed documents, where detoasting the
-- stored document dominates, so the guard stays.
--
-- What is pinned: both statement shapes write what the view computes, the
-- counts of a mixed move + rename, the rows journaled, and that a fan-out path
-- always names an object the view builds (other embeds are recomputed), so the
-- guard never skips a row jsonb_set could not patch.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/direct_patch/regress_fanout_guard.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'fanout_guard FAIL: %', what; END IF; END $$;

CREATE TABLE tb_cat (pk_cat int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     name text NOT NULL, code text NOT NULL DEFAULT 'c');
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_cat int REFERENCES tb_cat, title text NOT NULL);
CREATE INDEX ON tb_item (fk_cat);
INSERT INTO tb_cat (pk_cat, name) SELECT g, 'cat ' || g FROM generate_series(1, 10) g;
INSERT INTO tb_item (pk_item, fk_cat, title) SELECT g, 1 + g % 10, 't' FROM generate_series(1, 100) g;
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, i.fk_cat,
           jsonb_build_object('title', i.title,
                              'category', jsonb_build_object('name', c.name, 'code', c.code)) AS data
    FROM tb_item i JOIN tb_cat c ON c.pk_cat = i.fk_cat $$);

-- 1. Both statement shapes (one key: `= $1`; several: joined from the
--    arrays), one path and two.
UPDATE tb_cat SET name = 'one' WHERE pk_cat = 2;
UPDATE tb_cat SET name = 'two', code = 'z' WHERE pk_cat IN (3, 4);
SELECT assert_fresh('tv_item', 'pk_item', 'one key, then two keys with two paths');
SELECT must((SELECT count(*) FROM tv_item WHERE data #>> '{category,name}' = 'two'
                                             AND data #>> '{category,code}' = 'z') = 20,
            'the two-path write reached 20 children');

-- 2. The guard still leaves alone what it should, and counts as before: a write
--    that moves two items into a category and renames it in one statement
--    patches the category's ten children and recomputes the two moved ones.
SELECT tviews.pg_tviews_stats_reset();
WITH moved AS (UPDATE tb_item SET fk_cat = 7 WHERE pk_item IN (14, 24) RETURNING 1)
UPDATE tb_cat SET name = 'seven' WHERE pk_cat = 7;
SELECT must((view_recomputes, patch_applied, noop_skipped) = (2, 10, 0),
            format('move + rename: %s recomputes, %s patches, %s no-ops',
                   view_recomputes, patch_applied, noop_skipped))
FROM tviews.stats WHERE entity = 'item';
SELECT assert_fresh('tv_item', 'pk_item', 'move + rename');

-- 3. Rows the write leaves as they are are not journaled.
BEGIN;
UPDATE tb_cat SET name = 'eight' WHERE pk_cat = 8;
SELECT must(jsonb_array_length(r->'updated') = 10, 'a rename journals its ten children')
FROM tviews.pg_tviews_flush_and_report(include_data => false) r;
COMMIT;

-- 4. A fan-out path always names an object the view builds: shapes where the
--    embedded object may be missing (CASE, jsonb_strip_nulls, COALESCE) are not
--    patched in place but recomputed (the 8 items of category 5), and stay
--    fresh. tviews.stats counts a transaction's refreshes once it commits, so
--    each step is its own statement.
CREATE FUNCTION make_maybe(def text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF to_regclass('tv_maybe') IS NOT NULL THEN PERFORM tviews.pg_tviews_drop('tv_maybe'); END IF;
    PERFORM tviews.pg_tviews_create('tv_maybe', format($q$
        SELECT i.pk_item AS pk_maybe, i.id, i.fk_cat, %s AS data
        FROM tb_item i LEFT JOIN tb_cat c ON c.pk_cat = i.fk_cat $q$, def));
    PERFORM tviews.pg_tviews_stats_reset();
END $$;
CREATE FUNCTION recomputed(shape text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE st record;
BEGIN
    SELECT * INTO st FROM tviews.stats WHERE entity = 'maybe';
    PERFORM must(st.patch_applied = 0 AND st.view_recomputes = 8,
                 format('%s: %s patches, %s recomputes', shape, st.patch_applied, st.view_recomputes));
    PERFORM assert_fresh('tv_maybe', 'pk_maybe', shape);
END $$;

SELECT make_maybe($$jsonb_build_object('category',
    CASE WHEN c.name LIKE 'x%' THEN NULL ELSE jsonb_build_object('name', c.name) END)$$);
UPDATE tb_cat SET name = name || '+' WHERE pk_cat = 5;
SELECT recomputed('CASE');

SELECT make_maybe($$jsonb_strip_nulls(jsonb_build_object('category',
    jsonb_build_object('name', c.name)))$$);
UPDATE tb_cat SET name = name || '+' WHERE pk_cat = 5;
SELECT recomputed('jsonb_strip_nulls');

SELECT make_maybe($$jsonb_build_object('category',
    COALESCE(jsonb_build_object('name', c.name), '{}'))$$);
UPDATE tb_cat SET name = name || '+' WHERE pk_cat = 5;
SELECT recomputed('COALESCE');

\echo 'fan-out guard: PASS'

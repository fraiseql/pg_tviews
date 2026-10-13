-- Regression test (#238): the fan-out UPDATE builds each patched document once.
--
-- A write to a column that child TVIEWs copy (tb_cat.name into every tv_item
-- row of the category) patches the children in place: one UPDATE setting
-- `data = jsonb_set(t.data, path, value)`. Its guard, which leaves alone the
-- rows already holding the value, compared the documents:
-- `t.data IS DISTINCT FROM jsonb_set(t.data, path, value)`, so every row built
-- its new document twice, once in the filter and once in SET, each a full copy
-- of the document. The guard compares the values at the paths instead.
--
-- The plans are read through auto_explain (VERBOSE, nested statements) in a
-- second session on this database: `jsonb_set` must appear in the patched
-- rows' new values only, never in a filter.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/direct_patch/regress_fanout_single_eval.sql
-- known-failing: #238

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#238 FAIL: %', what; END IF; END $$;

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

-- 1. Both statement shapes (one key: `= $1`; several: joined from the arrays),
--    one path and two, planned in another session with auto_explain.
\setenv PGTV_DB :DBNAME
\set plans `psql -X -q -d "$PGTV_DB" -c "LOAD 'auto_explain'" -c "SET auto_explain.log_min_duration = 0" -c "SET auto_explain.log_nested_statements = on" -c "SET auto_explain.log_verbose = on" -c "SET auto_explain.log_level = notice" -c "UPDATE tb_cat SET name = 'one' WHERE pk_cat = 2" -c "UPDATE tb_cat SET name = 'two', code = 'z' WHERE pk_cat IN (3, 4)" 2>&1 | grep -E 'Filter:|Output:' | grep -F 'jsonb_set(' | sed -E 's/^ *([A-Za-z ]*Filter|Output).*/\1/' | sort | uniq -c | tr -s ' \n' '  '`
SELECT :'plans' AS plans \gset
SELECT must(:'plans' LIKE '%Output%', 'auto_explain showed no fan-out UPDATE: ' || :'plans');
SELECT must(:'plans' NOT LIKE '%Filter%',
            'a fan-out UPDATE builds the patched document in its filter: ' || :'plans');
SELECT assert_fresh('tv_item', 'pk_item', 'the planned writes');
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

\echo 'fan-out single evaluation (#238): PASS'

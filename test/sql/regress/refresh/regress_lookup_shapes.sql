-- Regression test: statements that look rows up by several keys take one of a
-- few shapes, each planned once per backend.
--
-- A lookup by several keys (a read set, a fan-out patch and its cross-check,
-- parent discovery, a report) took the keys as an array and was planned at
-- every execution: a generic plan of an array parameter assumes ten keys (#235).
-- Up to 128 keys now take the next width of 1, 2, 4, … or 128
-- parameters, padded by repeating the last key, from a generic plan that
-- estimates that many keys. More keys are still an array, planned at each
-- execution.
--
-- Checked: no lookup is planned at execution up to 128 keys
-- (`per_execution_plans`), every TVIEW holds what its view computes for every
-- key count (padding writes nothing twice and misses nothing), and the plans
-- kept stay bounded.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_lookup_shapes.sql
--
-- expect-output: lookup shapes: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'lookup shapes FAIL: %', what; END IF; END $$;
CREATE FUNCTION stat(name text) RETURNS bigint LANGUAGE sql AS
    $$ SELECT (tviews.pg_tviews_show_queue()->>name)::bigint $$;

-- 200 categories of 2 items. tv_item copies its category's name (a fan-out
-- patch by fk_cat), tv_card embeds tv_cat (parent discovery by fk_cat), and
-- tv_tally shows each item with its category's count of current items (a link:
-- a soft-delete rewrites the category's items, and locks their read sets).
-- tv_badge is keyed by a uuid.
CREATE TABLE tb_cat (pk_cat int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     name text NOT NULL);
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_cat int NOT NULL REFERENCES tb_cat, title text NOT NULL,
                      deleted_at timestamptz);
CREATE INDEX ON tb_item (fk_cat);
INSERT INTO tb_cat (pk_cat, name) SELECT g, 'cat ' || g FROM generate_series(1, 200) g;
INSERT INTO tb_item (pk_item, fk_cat, title)
    SELECT (c - 1) * 2 + i, c, 'item ' || ((c - 1) * 2 + i)
    FROM generate_series(1, 200) c, generate_series(1, 2) i;
SELECT tviews.pg_tviews_create('tv_cat', $$
    SELECT c.pk_cat, c.id, jsonb_build_object('name', c.name) AS data FROM tb_cat c $$);
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, i.fk_cat,
           jsonb_build_object('title', i.title, 'category', jsonb_build_object('name', c.name)) AS data
    FROM tb_item i JOIN tb_cat c ON c.pk_cat = i.fk_cat $$);
SELECT tviews.pg_tviews_create('tv_card', $$
    SELECT i.pk_item AS pk_card, i.id, i.fk_cat,
           jsonb_build_object('title', i.title, 'category', c.data) AS data
    FROM tb_item i JOIN tv_cat c ON c.pk_cat = i.fk_cat $$);
SELECT tviews.pg_tviews_create('tv_tally', $$
    SELECT i.pk_item AS pk_tally, i.id,
           jsonb_build_object('title', i.title, 'current', COALESCE(n.n, 0)) AS data
    FROM tb_item i
    LEFT JOIN (SELECT fk_cat, count(*) AS n FROM tb_item WHERE deleted_at IS NULL
               GROUP BY fk_cat) n ON n.fk_cat = i.fk_cat $$);
SELECT tviews.pg_tviews_create('tv_badge', $$
    SELECT DISTINCT ON (i.id) i.id, i.pk_item AS pk_badge,
           jsonb_build_object('category', c.name, 'deleted', i.deleted_at IS NOT NULL) AS data
    FROM tb_item i JOIN tb_cat c ON c.pk_cat = i.fk_cat ORDER BY i.id $$);
ANALYZE tb_cat, tb_item, tv_cat, tv_item, tv_card, tv_tally, tv_badge;

CREATE FUNCTION all_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM assert_fresh('tv_cat', 'pk_cat', label);
    PERFORM assert_fresh('tv_item', 'pk_item', label);
    PERFORM assert_fresh('tv_card', 'pk_card', label);
    PERFORM assert_fresh('tv_tally', 'pk_tally', label);
    PERFORM assert_fresh('tv_badge', 'id', label);
END $$;

-- The writes of n keys: a rename of n categories (fan-out patches, parent
-- discovery), and a soft-delete then a restore of one item in each of n
-- categories (link changes: every item of those categories recomputed, its read
-- set locked). `round` makes each round's names new.
CREATE FUNCTION writes(n int, round int) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_cat SET name = format('cat %s r%s', pk_cat, round) WHERE pk_cat <= n;
    UPDATE tb_item SET deleted_at = now() WHERE fk_cat <= n AND pk_item % 2 = 1;
    UPDATE tb_item SET deleted_at = NULL WHERE fk_cat <= n AND pk_item % 2 = 1;
END $$;

-- Warm every statement once.
SELECT writes(1, 0), writes(3, 0);

-- 1. Up to 128 keys, nothing is planned at execution (n categories reach 2n
--    items).
DO $$
DECLARE n int; before bigint;
BEGIN
    FOREACH n IN ARRAY ARRAY[1, 2, 3, 4, 5, 8, 9, 31, 33, 63, 64] LOOP
        before := stat('per_execution_plans');
        PERFORM writes(n, 1);
        PERFORM must(stat('per_execution_plans') = before,
                     format('%s keys: %s lookups planned at execution', n,
                            stat('per_execution_plans') - before));
        PERFORM all_fresh(format('%s keys', n));
    END LOOP;
END $$;

-- 2. More keys (130 items and up) are an array planned at each execution, and still right.
DO $$
DECLARE n int; before bigint;
BEGIN
    FOREACH n IN ARRAY ARRAY[65, 129, 200] LOOP
        before := stat('per_execution_plans');
        PERFORM writes(n, 2);
        PERFORM must(stat('per_execution_plans') > before,
                     format('%s keys: no lookup planned at execution', n));
        PERFORM all_fresh(format('%s keys', n));
    END LOOP;
END $$;

-- 3. Padding writes no row twice: a rename of three categories (padded to four
--    keys) patches their three tv_cat rows, their six tv_item rows and the six
--    tv_card rows embedding them, once each.
DO $$
DECLARE before bigint := stat('direct_patches_applied');
BEGIN
    UPDATE tb_cat SET name = name || ' once' WHERE pk_cat IN (7, 8, 9);
    PERFORM must(stat('direct_patches_applied') - before = 15,
                 format('three categories: %s rows patched, not 15',
                        stat('direct_patches_applied') - before));
    PERFORM all_fresh('three categories, padded');
END $$;

-- 4. The plans kept are bounded: a key count of every width once more
--    prepares nothing.
SELECT stat('plans_prepared') AS warm \gset
DO $$
DECLARE n int;
BEGIN
    FOREACH n IN ARRAY ARRAY[1, 2, 3, 5, 9, 17, 33, 65, 200] LOOP
        PERFORM writes(n, 3);
    END LOOP;
END $$;
SELECT stat('plans_prepared') AS after_all \gset
DO $$
DECLARE n int;
BEGIN
    FOREACH n IN ARRAY ARRAY[1, 2, 3, 5, 9, 17, 33, 65, 200] LOOP
        PERFORM writes(n, 4);
    END LOOP;
END $$;
SELECT must(stat('plans_prepared') = :after_all,
            format('a second round prepared %s statements', stat('plans_prepared') - :after_all));
SELECT all_fresh('every key count');

-- 5. A session's plan_cache_mode doesn't decide how a write plans: a refresh
--    runs under `auto`, so each statement is planned as it was kept (a
--    session's force_generic_plan would plan an array parameter for ten keys,
--    #235). Seen from the backing view, which renders the setting.
CREATE TABLE tb_mode (pk_mode int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v int);
INSERT INTO tb_mode VALUES (1, default, 0);
SELECT tviews.pg_tviews_create('tv_mode', $$
    SELECT pk_mode, id, jsonb_build_object('v', v, 'mode', current_setting('plan_cache_mode')) AS data
    FROM tb_mode $$);
SET plan_cache_mode = force_generic_plan;
SET pg_tviews.direct_patch_enabled = off;  -- recompute the row from its view
UPDATE tb_mode SET v = 1;
RESET pg_tviews.direct_patch_enabled;
SELECT must(data ->> 'mode' = 'auto', format('a write planned under %s', data ->> 'mode')) FROM tv_mode;
SELECT must(current_setting('plan_cache_mode') = 'force_generic_plan',
            'the write changed the session''s plan_cache_mode');
SELECT writes(9, 5), writes(65, 5);
SET plan_cache_mode = force_custom_plan;
SELECT writes(9, 6), writes(65, 6);
RESET plan_cache_mode;
SELECT all_fresh('writes under forced plan cache modes');

SELECT 'lookup shapes: PASS' AS result;
-- expect-output: lookup shapes: PASS

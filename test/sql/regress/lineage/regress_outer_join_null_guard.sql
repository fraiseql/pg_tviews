-- A table's guard (#284) holds only conditions that a row it fails can never
-- change the view through. `col IS NULL` over a column an outer join may
-- NULL-extend is not one: a NULL-extended row meets it, a real row of the
-- table fails it, and adding or removing that row changes the view. Such a
-- condition is not recorded as a guard, so every write to the table refreshes;
-- a strict condition (`=`, `IN`) and `IS NULL` in the outer join's own
-- condition still are.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_outer_join_null_guard.sql
-- expect-output: outer join null guard: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'outer join null guard: %', what; END IF; END $$;
-- Whether `tview`'s rows are its view's.
CREATE FUNCTION in_sync(tview text) RETURNS boolean LANGUAGE plpgsql AS $$
DECLARE differ boolean;
BEGIN
    EXECUTE format('SELECT EXISTS (SELECT 1 FROM public.%1$I t FULL JOIN tviews.public__%1$I v '
                   'USING (%2$I) WHERE t.data IS DISTINCT FROM v.data)',
                   tview, 'pk_' || substr(tview, 4)) INTO differ;
    RETURN NOT differ;
END $$;
-- The guard stored for `tbl` in `tview`'s plan.
CREATE FUNCTION guard_of(tview text, tbl regclass) RETURNS text LANGUAGE sql AS $$
    SELECT t->>'guard' FROM tviews.pg_tview_meta m, jsonb_array_elements(m.plan->'tables') t
    WHERE m.entity = substr(tview, 4) AND (t->>'relid')::oid = tbl $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
CREATE TABLE tb_notice (pk_notice int PRIMARY KEY, kind text, fk_item int);
CREATE TABLE tb_extra (pk_extra int PRIMARY KEY, note text);
INSERT INTO tb_item (pk_item, title) SELECT g, 'i' || g FROM generate_series(1, 5) g;
INSERT INTO tb_extra SELECT g, 'e' || g FROM generate_series(1, 5) g;

-- An anti-join on a flag row.
SELECT tviews.pg_tviews_create('tv_open', $$
    SELECT i.pk_item AS pk_open, i.id, jsonb_build_object('title', i.title) AS data
    FROM tb_item i LEFT JOIN tb_notice n ON n.kind = 'closed'
    WHERE n.pk_notice IS NULL $$, '{"uncascaded_policy": "full_refresh"}');
-- The same test in an inner join above the outer join.
SELECT tviews.pg_tviews_create('tv_open_joined', $$
    SELECT i.pk_item AS pk_open_joined, i.id, jsonb_build_object('note', e.note) AS data
    FROM (tb_item i LEFT JOIN tb_notice n ON n.kind = 'closed')
    JOIN tb_extra e ON e.pk_extra = i.pk_item AND n.pk_notice IS NULL $$,
    '{"uncascaded_policy": "full_refresh"}');
-- The test in an outer join's condition, over a column a nested outer join
-- NULL-extends.
SELECT tviews.pg_tviews_create('tv_open_nested', $$
    SELECT i.pk_item AS pk_open_nested, i.id, jsonb_build_object('extra', e.pk_extra) AS data
    FROM tb_item i
    LEFT JOIN (tb_extra e LEFT JOIN tb_notice n ON n.kind = 'closed')
           ON e.pk_extra = i.pk_item AND n.pk_notice IS NULL $$,
    '{"uncascaded_policy": "full_refresh"}');
-- Controls: `IS NULL` in the outer join's own condition, and a strict guard.
SELECT tviews.pg_tviews_create('tv_unkinded', $$
    SELECT i.pk_item AS pk_unkinded, i.id, jsonb_build_object('notice', n.pk_notice) AS data
    FROM tb_item i LEFT JOIN tb_notice n ON n.kind IS NULL $$,
    '{"uncascaded_policy": "full_refresh"}');
SELECT tviews.pg_tviews_create('tv_closed', $$
    SELECT i.pk_item AS pk_closed, i.id,
           jsonb_build_object('closed', (SELECT count(*) FROM tb_notice n WHERE n.kind = 'closed')) AS data
    FROM tb_item i $$, '{"uncascaded_policy": "full_refresh"}');

SELECT must(guard_of(t, 'tb_notice') IS NULL OR guard_of(t, 'tb_notice') NOT LIKE '%IS NULL%',
            format('%s keeps an IS NULL guard over a NULL-extended column: %s', t, guard_of(t, 'tb_notice')))
FROM unnest(ARRAY['tv_open', 'tv_open_joined', 'tv_open_nested']) t;
SELECT must(guard_of('tv_unkinded', 'tb_notice') LIKE '%IS NULL%',
            format('the outer join''s own IS NULL lost its guard: %s', guard_of('tv_unkinded', 'tb_notice')));
SELECT must(guard_of('tv_closed', 'tb_notice') LIKE '%=%',
            format('a strict guard was lost: %s', guard_of('tv_closed', 'tb_notice')));

-- The flag row appears, then goes.
INSERT INTO tb_notice VALUES (1, 'closed', NULL);
SELECT must(in_sync(t), format('%s after the flag row appeared', t))
FROM unnest(ARRAY['tv_open', 'tv_open_joined', 'tv_open_nested', 'tv_unkinded', 'tv_closed']) t;
DELETE FROM tb_notice;
SELECT must(in_sync(t), format('%s after the flag row went', t))
FROM unnest(ARRAY['tv_open', 'tv_open_joined', 'tv_open_nested', 'tv_unkinded', 'tv_closed']) t;
-- A notice without a kind (one: each item joins it).
INSERT INTO tb_notice VALUES (2, NULL, 3);
SELECT must(in_sync('tv_unkinded'), 'tv_unkinded after a notice without a kind');

SELECT 'outer join null guard: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_notice, tb_extra, tb_item;
DROP FUNCTION must(boolean, text), in_sync(text), guard_of(text, regclass);

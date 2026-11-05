-- The constants a stored plan holds are rendered under the settings every
-- refresh parses them under, whatever the session that derives the plan set.
--
-- A constant lookup (`WHERE e.day = '2024-12-25'`) is stored as a guard: a write
-- refreshes in full only when a row matches it. Its constant is rendered when the
-- plan is derived (create, create_or_replace, reregister, a column rename) and
-- parsed back at each write under the pinned settings. Rendered under the
-- deriving session's DateStyle, extra_float_digits or IntervalStyle, it would
-- fail to parse or mean another value, and writes would fail or miss it.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_guard_render.sql
-- expect-output: guard constants: rendered under the pinned settings

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
SELECT regress_roles('regress_gr');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'guard render: %', what; END IF; END $$;

SET ROLE regress_gr_owner_b;
CREATE TABLE public.tb_day (pk_day int PRIMARY KEY, day date, note text);
CREATE TABLE public.tb_amount (pk_amount int PRIMARY KEY, amount float8);
CREATE TABLE public.tb_span (pk_span int PRIMARY KEY, span interval);
GRANT SELECT, TRIGGER ON public.tb_day, public.tb_amount, public.tb_span TO regress_gr_owner_a;
GRANT SELECT, INSERT, UPDATE ON public.tb_day, public.tb_amount, public.tb_span
    TO regress_gr_writer;
RESET ROLE;

SET ROLE regress_gr_owner_a;
CREATE TABLE public.tb_order (pk_order int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                              name text);
INSERT INTO public.tb_order (pk_order, name) SELECT g, 'o' || g FROM generate_series(1, 3) g;
-- The deriving session's settings differ from the pinned ones.
SET DateStyle = 'SQL, DMY';
SET extra_float_digits = -15;
SET IntervalStyle = 'sql_standard';
SELECT tviews.pg_tviews_create('public.tv_order', $q$
    SELECT o.pk_order, o.id, jsonb_build_object('id', o.id,
        'christmas', (SELECT count(*) FROM public.tb_day e WHERE e.day = '2024-12-25'::date),
        'tiny', (SELECT count(*) FROM public.tb_amount e
                 WHERE e.amount = '0.30000000000000004'::float8),
        'back', (SELECT count(*) FROM public.tb_span e
                 WHERE e.span = '-1 days -02:00:00'::interval)) AS data
    FROM public.tb_order o $q$, '{"uncascaded_policy": "full_refresh"}');
RESET DateStyle;
RESET extra_float_digits;
RESET IntervalStyle;
RESET ROLE;

CREATE FUNCTION in_sync() RETURNS boolean LANGUAGE sql AS $$
    SELECT NOT EXISTS (SELECT 1 FROM public.tv_order t
                       FULL JOIN tviews.public__tv_order v USING (pk_order)
                       WHERE t.data IS DISTINCT FROM v.data) $$;
CREATE FUNCTION guards_iso() RETURNS boolean LANGUAGE sql AS $$
    SELECT count(*) FILTER (WHERE g LIKE '%2024-12-25%' OR g LIKE '%0.30000000000000004%'
                               OR g LIKE '%-1 days -02:00:00%') = 3
    FROM tviews.pg_tview_meta m, jsonb_path_query(m.plan, '$.tables[*].guard') j(g0),
         LATERAL (SELECT g0 #>> '{}') x(g)
    WHERE m.entity = 'order' $$;

SELECT must(guards_iso(), 'created under the session''s settings');

-- A writer under the default settings writes rows each guard matches.
SET ROLE regress_gr_writer;
INSERT INTO public.tb_day VALUES (1, '2024-12-25', 'x'), (2, '2024-12-24', 'y');
INSERT INTO public.tb_amount VALUES (1, 0.30000000000000004), (2, 0.5);
INSERT INTO public.tb_span VALUES (1, '-1 days -02:00:00'), (2, '1 day');
RESET ROLE;
SELECT must(in_sync(), 'a row matching a guard refreshed the TVIEW');

-- A column rename by the table's owner, under its own settings, re-derives the
-- plan: the constants are rendered the same.
SET ROLE regress_gr_owner_b;
SET DateStyle = 'SQL, DMY';
SET extra_float_digits = -15;
SET IntervalStyle = 'sql_standard';
ALTER TABLE public.tb_day RENAME COLUMN note TO remark;
RESET DateStyle;
RESET extra_float_digits;
RESET IntervalStyle;
RESET ROLE;
SELECT must(guards_iso(), 're-derived under the renaming role''s settings');
SET ROLE regress_gr_writer;
UPDATE public.tb_day SET day = '2024-12-25' WHERE pk_day = 2;
UPDATE public.tb_amount SET amount = 0.30000000000000004 WHERE pk_amount = 2;
UPDATE public.tb_span SET span = '-1 days -02:00:00' WHERE pk_span = 2;
RESET ROLE;
SELECT must(in_sync(), 'a row matching a guard refreshed the TVIEW after the rename');

SELECT 'guard constants: rendered under the pinned settings' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_day, public.tb_amount, public.tb_span, public.tb_order;
SELECT regress_drop_roles('regress_gr');

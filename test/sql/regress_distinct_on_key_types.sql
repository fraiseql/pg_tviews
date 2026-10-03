-- A DISTINCT ON key of any type names the TVIEW's rows (ADR 0169): a quoted
-- mixed-case column, a uuid, a numeric, a date. Before, a key the trigger could
-- not read as text, uuid, int4 or int8 skipped every write with a WARNING, and a
-- quoted key was looked up under the wrong name.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_distinct_on_key_types.sql
--
-- expect-output: DISTINCT ON key types: PASS
-- reject-output: skipping refresh

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

-- One table per key type, each with versions of rows grouped by that key.
DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['bycode', 'bygrp', 'byprice', 'byday'] LOOP
        EXECUTE format($q$
            CREATE TABLE %1$I (%2$I int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
              id uuid NOT NULL DEFAULT gen_random_uuid(), "Code" text NOT NULL, grp uuid NOT NULL,
              price numeric NOT NULL, day date NOT NULL, rev int NOT NULL, label text);
            INSERT INTO %1$I ("Code", grp, price, day, rev, label)
              SELECT 'C' || (g %% 3), ('00000000-0000-0000-0000-00000000000' || (g %% 3))::uuid,
                     (g %% 3) + 0.5, DATE '2026-01-01' + (g %% 3), g, 'l' || g
              FROM generate_series(1, 9) g $q$, 'tb_' || t, 'pk_' || t);
    END LOOP;
END $$;

SELECT pg_tviews_create('tv_bycode', $$
  SELECT DISTINCT ON (i."Code") i.pk_bycode, i.id, i."Code",
         jsonb_build_object('label', i.label) AS data
  FROM tb_bycode i ORDER BY i."Code", i.rev DESC $$);
SELECT pg_tviews_create('tv_bygrp', $$
  SELECT DISTINCT ON (i.grp) i.pk_bygrp, i.id, i.grp,
         jsonb_build_object('label', i.label) AS data
  FROM tb_bygrp i ORDER BY i.grp, i.rev DESC $$);
SELECT pg_tviews_create('tv_byprice', $$
  SELECT DISTINCT ON (i.price) i.pk_byprice, i.id, i.price,
         jsonb_build_object('label', i.label) AS data
  FROM tb_byprice i ORDER BY i.price, i.rev DESC $$);
SELECT pg_tviews_create('tv_byday', $$
  SELECT DISTINCT ON (i.day) i.pk_byday, i.id, i.day,
         jsonb_build_object('label', i.label) AS data
  FROM tb_byday i ORDER BY i.day, i.rev DESC $$);

-- The same statement on every table.
CREATE FUNCTION on_all(stmt text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['tb_bycode', 'tb_bygrp', 'tb_byprice', 'tb_byday'] LOOP
        EXECUTE format(stmt, t);
    END LOOP;
END $$;

CREATE FUNCTION check_all(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM assert_fresh('tv_bycode', 'Code', label);
    PERFORM assert_fresh('tv_bygrp', 'grp', label);
    PERFORM assert_fresh('tv_byprice', 'price', label);
    PERFORM assert_fresh('tv_byday', 'day', label);
END $$;

SELECT on_all($$UPDATE %I SET label = label || '!'$$);
SELECT check_all('an UPDATE of every row');
SELECT on_all($$UPDATE %I SET label = 'one' WHERE rev = 9$$);
SELECT check_all('an UPDATE of one winner');
SELECT on_all($$UPDATE %I SET "Code" = 'C9', grp = '00000000-0000-0000-0000-000000000009',
                   price = 9.5, day = DATE '2026-12-31' WHERE rev = 8$$);
SELECT check_all('a winner moving to new keys');
SELECT on_all($$INSERT INTO %I ("Code", grp, price, day, rev, label)
  VALUES ('C0', '00000000-0000-0000-0000-000000000000', 0.5, DATE '2026-01-01', 99, 'new')$$);
SELECT check_all('a new winner');
SELECT on_all($$DELETE FROM %I WHERE rev IN (99, 9)$$);
SELECT check_all('winners deleted');

\echo 'DISTINCT ON key types: PASS'

-- A new TVIEW's columns have exactly the types of its backing view's columns:
-- enums, domains, composites, arrays of them, types in other or quoted schemas,
-- and typmods. An existing TVIEW stored with other types keeps them until
-- pg_tviews_create_or_replace() retypes it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_tview_column_types.sql
--
-- expect-output: tview column types: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE SCHEMA app;
CREATE SCHEMA "Other";
CREATE TYPE app.mood AS ENUM ('sad', 'ok', 'happy');   -- not alphabetical
CREATE DOMAIN app.posint AS int CHECK (VALUE > 0);
CREATE DOMAIN app.shorttxt AS text CHECK (length(VALUE) < 10);
CREATE TYPE app.pair AS (a int, b text);
CREATE TYPE "Other"."Weird Type" AS ENUM ('x', 'y');

-- ── behaviour: a bit(4) column ──────────────────────────────────────────────
CREATE TABLE tb_bits (pk_bits bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      flags bit(4) NOT NULL);
INSERT INTO tb_bits (pk_bits, flags) VALUES (1, B'1010');
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create('tv_bits', $q$
        SELECT pk_bits, id, flags, jsonb_build_object('n', pk_bits) AS data FROM tb_bits $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION 'item 7 FAIL: a bit(4) column fails the create: %', SQLERRM;
END $$;

-- ── every column has the view's type ────────────────────────────────────────
CREATE TABLE tb_typed (pk_typed bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       mood app.mood, n app.posint, short app.shorttxt, p app.pair,
                       moods app.mood[], weird "Other"."Weird Type",
                       code varchar(5), amount numeric(6,2), tag char(3), flags bit(4));
INSERT INTO tb_typed (pk_typed, mood, n, short, p, moods, weird, code, amount, tag, flags) VALUES
    (1, 'happy', 1, 'a', ROW(1, 'x'), '{sad,ok}', 'x', 'abc', 1.5, 'ab', B'0001'),
    (2, 'sad',   2, 'b', ROW(2, 'y'), '{happy}',  'y', 'de',  2.25, 'cd', B'0010'),
    (3, 'ok',    3, 'c', ROW(3, 'z'), '{}',       'x', 'f',   3,    'e',  B'0100');
SELECT pg_tviews_create('tv_typed', $$
    SELECT pk_typed, id, mood, n, short, p, moods, weird, code, amount, tag, flags,
           jsonb_build_object('mood', mood) AS data
    FROM tb_typed $$);

CREATE FUNCTION type_drift(tv regclass, v regclass) RETURNS text LANGUAGE sql AS $$
    SELECT string_agg(format('%s is %s, view has %s', va.attname,
                             format_type(ta.atttypid, ta.atttypmod),
                             format_type(va.atttypid, va.atttypmod)), '; ')
    FROM pg_attribute va
    JOIN pg_attribute ta ON ta.attrelid = tv AND ta.attname = va.attname
    WHERE va.attrelid = v AND va.attnum > 0 AND NOT va.attisdropped
      AND va.attname NOT IN ('pk_typed', 'pk_retyped', 'pk_bits')   -- the key is bigint by design
      AND (ta.atttypid, ta.atttypmod) IS DISTINCT FROM (va.atttypid, va.atttypmod) $$;

DO $$ BEGIN
    IF type_drift('tv_typed', 'v_typed') IS NOT NULL THEN
        RAISE EXCEPTION 'item 7 FAIL: tv_typed: %', type_drift('tv_typed', 'v_typed');
    END IF;
END $$;

-- ── what the real types give back ───────────────────────────────────────────
DO $$ BEGIN
    IF (SELECT array_agg(pk_typed ORDER BY mood) FROM tv_typed)
       IS DISTINCT FROM (SELECT array_agg(pk_typed ORDER BY mood) FROM v_typed) THEN
        RAISE EXCEPTION 'item 7 FAIL: ORDER BY mood differs from the view';
    END IF;
    IF (SELECT pk_typed FROM tv_typed WHERE mood = 'happy'::app.mood) <> 1
       OR (SELECT (p).b FROM tv_typed WHERE pk_typed = 2) <> 'y' THEN
        RAISE EXCEPTION 'item 7 FAIL: tv_typed columns do not behave as their types';
    END IF;
END $$;
UPDATE tb_typed SET mood = 'ok', amount = 9.99 WHERE pk_typed = 1;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM tv_typed t JOIN v_typed v USING (pk_typed)
               WHERE (t.mood, t.amount, t.p, t.moods) IS DISTINCT FROM (v.mood, v.amount, v.p, v.moods)) THEN
        RAISE EXCEPTION 'item 7 FAIL: tv_typed stale after an UPDATE';
    END IF;
END $$;

-- ── D3: an existing TVIEW keeps its types until create_or_replace ──────────
CREATE TABLE tb_retyped (pk_retyped bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         mood app.mood, amount numeric(6,2));
INSERT INTO tb_retyped (pk_retyped, mood, amount) VALUES (1, 'happy', 1.25), (2, 'sad', 2);
SELECT pg_tviews_create('tv_retyped', $$
    SELECT pk_retyped, id, mood, amount, jsonb_build_object('mood', mood) AS data FROM tb_retyped $$);
-- As an earlier release stored them.
ALTER TABLE tv_retyped ALTER COLUMN mood TYPE text, ALTER COLUMN amount TYPE numeric;
SELECT count(*) FROM pg_tviews_reregister_all();
DO $$ BEGIN
    IF type_drift('tv_retyped', 'v_retyped') IS NULL THEN
        RAISE EXCEPTION 'item 7 FAIL: re-registration retyped an existing TVIEW';
    END IF;
END $$;
DO $$
DECLARE r text := tviews.pg_tviews_create_or_replace('tv_retyped', $q$
    SELECT pk_retyped, id, mood, amount, jsonb_build_object('mood', mood) AS data FROM tb_retyped $q$);
BEGIN
    IF r <> 'altered' OR type_drift('tv_retyped', 'v_retyped') IS NOT NULL THEN
        RAISE EXCEPTION 'item 7 FAIL: create_or_replace returned % and left %', r,
            type_drift('tv_retyped', 'v_retyped');
    END IF;
    IF (SELECT array_agg(pk_retyped ORDER BY mood) FROM tv_retyped) <> ARRAY[2, 1]::bigint[] THEN
        RAISE EXCEPTION 'item 7 FAIL: the retyped column lost its values';
    END IF;
END $$;

-- ── DROP TYPE … CASCADE drops the view, and with it the whole TVIEW ─────────
CREATE TYPE app.color AS ENUM ('red', 'blue');
CREATE TABLE tb_car (pk_car bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     c app.color, name text);
INSERT INTO tb_car (pk_car, c, name) VALUES (1, 'red', 'a');
SELECT pg_tviews_create('tv_car', $$
    SELECT pk_car, id, c, jsonb_build_object('name', name) AS data FROM tb_car $$);
DROP TYPE app.color CASCADE;
DO $$ BEGIN
    IF to_regclass('tv_car') IS NOT NULL
       OR EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'car')
       OR EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'tb_car'::regclass
                  AND tgname LIKE 'trg_tview_%') THEN
        RAISE EXCEPTION 'item 7 FAIL: DROP TYPE CASCADE left part of tv_car behind';
    END IF;
END $$;
UPDATE tb_car SET name = 'b';   -- the base table still works

\echo 'tview column types: PASS'


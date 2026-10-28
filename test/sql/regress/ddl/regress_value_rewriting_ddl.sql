-- DDL that changes the values a TVIEW reads without firing a trigger refreshes
-- the TVIEWs reading them, in the statement:
--
--   * ALTER TABLE … ALTER COLUMN … SET EXPRESSION recomputes a stored generated
--     column, or redefines a virtual one;
--   * ALTER TYPE … RENAME VALUE relabels an enum: every column of the type shows
--     the new label.
--
-- DROP EXPRESSION keeps the values: nothing to refresh. And the health check
-- reports a pg_tviews trigger switched to ENABLE REPLICA, which stops firing on
-- the primary.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_value_rewriting_ddl.sql
--
-- expect-output: value-rewriting DDL: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

SELECT current_setting('server_version_num')::int >= 170000 AS has_set_expression \gset
SELECT CASE WHEN current_setting('server_version_num')::int >= 180000 THEN 'VIRTUAL'
            ELSE 'STORED' END AS generated \gset

-- ── generated columns ────────────────────────────────────────────────────────
CREATE TABLE tb_gen (pk_gen bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     a int,
                     g int GENERATED ALWAYS AS (a * 2) STORED,
                     v int GENERATED ALWAYS AS (a * 3) :generated);
INSERT INTO tb_gen (pk_gen, a) VALUES (1, 10), (2, 20);
SELECT tviews.pg_tviews_create('tv_gen', $$
    SELECT pk_gen, id, jsonb_build_object('g', g, 'v', v) AS data FROM tb_gen $$);
-- a TVIEW reading only `a` is left alone by a change to g's expression
SELECT tviews.pg_tviews_create('tv_raw', $$
    SELECT pk_gen AS pk_raw, id, jsonb_build_object('a', a) AS data FROM tb_gen $$);

\if :has_set_expression
ALTER TABLE tb_gen ALTER COLUMN g SET EXPRESSION AS (a * 100);
SELECT assert_fresh('tv_gen', 'pk_gen', 'SET EXPRESSION of a stored generated column');
ALTER TABLE tb_gen ALTER COLUMN v SET EXPRESSION AS (a * 1000);
SELECT assert_fresh('tv_gen', 'pk_gen', 'SET EXPRESSION of a generated column');
SELECT assert_fresh('tv_raw', 'pk_raw', 'SET EXPRESSION of a column tv_raw does not read');
\endif
ALTER TABLE tb_gen ALTER COLUMN g DROP EXPRESSION;
UPDATE tb_gen SET a = a + 1;
SELECT assert_fresh('tv_gen', 'pk_gen', 'DROP EXPRESSION, then a write');

-- ── enum labels ──────────────────────────────────────────────────────────────
CREATE TYPE mood AS ENUM ('sad', 'ok', 'happy');
CREATE DOMAIN mood_d AS mood;
CREATE TABLE tb_feel (pk_feel bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      m mood, md mood_d, ms mood[]);
INSERT INTO tb_feel VALUES (1, DEFAULT, 'sad', 'sad', '{sad,ok}'), (2, DEFAULT, 'ok', 'ok', '{ok}');
SELECT tviews.pg_tviews_create('tv_feel', $$
    SELECT pk_feel, id, jsonb_build_object('m', m) AS data FROM tb_feel $$);
SELECT tviews.pg_tviews_create('tv_feel_d', $$
    SELECT pk_feel AS pk_feel_d, id, jsonb_build_object('m', md) AS data FROM tb_feel $$);
SELECT tviews.pg_tviews_create('tv_feel_a', $$
    SELECT pk_feel AS pk_feel_a, id, jsonb_build_object('m', ms) AS data FROM tb_feel $$);
-- the label in the definition itself
SELECT tviews.pg_tviews_create('tv_feel_lit', $$
    SELECT pk_feel AS pk_feel_lit, id, jsonb_build_object('sad', m = 'sad'::mood,
                                                          'label', 'sad'::mood) AS data FROM tb_feel $$);
ALTER TYPE mood RENAME VALUE 'sad' TO 'blue';
SELECT assert_fresh('tv_feel', 'pk_feel', 'RENAME VALUE of an enum a column holds');
SELECT assert_fresh('tv_feel_d', 'pk_feel_d', 'RENAME VALUE of the enum under a domain');
SELECT assert_fresh('tv_feel_a', 'pk_feel_a', 'RENAME VALUE of the enum of an array');
SELECT assert_fresh('tv_feel_lit', 'pk_feel_lit', 'RENAME VALUE of an enum the definition names');

-- ── triggers that won't fire on the primary ──────────────────────────────────
DO $$
DECLARE trg text;
BEGIN
    SELECT tgname INTO trg FROM pg_trigger
     WHERE tgrelid = 'tb_feel'::regclass AND tgname LIKE 'trg_tview_row_feel_%';
    EXECUTE format('ALTER TABLE tb_feel ENABLE REPLICA TRIGGER %I', trg);
END $$;
DO $$
DECLARE report text;
BEGIN
    SELECT pg_catalog.string_agg(c::text, ' ') INTO report FROM tviews.pg_tviews_health_check() c;
    IF report NOT LIKE '%trg_tview_row_feel_%tb_feel%ENABLE REPLICA%' THEN
        RAISE EXCEPTION 'value-rewriting DDL FAIL: the health check misses an ENABLE REPLICA trigger: %',
            report;
    END IF;
END $$;

\echo 'value-rewriting DDL: PASS'

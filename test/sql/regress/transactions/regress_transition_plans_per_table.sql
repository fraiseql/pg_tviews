-- Regression test: kept plans over transition tables are per table.
--
-- The statement triggers query a statement's changed rows as pg_tviews_old and
-- pg_tviews_new, whatever the table. Two tables can therefore produce the same
-- SQL text (same key column names, same mapping query) with different row
-- layouts: a plan kept for one must never run over the other's rows.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_transition_plans_per_table.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_thing (pk_thing int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       code text, other text);
-- Same key and lookup column names, other positions and other columns.
CREATE TABLE tb_x (id int PRIMARY KEY, code text, label text);
CREATE TABLE tb_y (weight numeric, note text, label text, code text, id int PRIMARY KEY);
INSERT INTO tb_thing SELECT g, gen_random_uuid(), 'c' || g, 'o' || g FROM generate_series(1, 6) g;
INSERT INTO tb_x SELECT g, 'c' || g, 'x' || g FROM generate_series(1, 6) g;
INSERT INTO tb_y SELECT g * 1.5, 'n', 'y' || g, 'c' || g, g FROM generate_series(1, 6) g;

-- Each table is read through a join on code, by a TVIEW of its own: the mapping
-- queries of tb_x and tb_y differ only in the TVIEW they key.
SELECT tviews.pg_tviews_create('tv_thing', $$
    SELECT t.pk_thing, t.id,
           jsonb_build_object('x', (SELECT x.label FROM tb_x x WHERE x.code = t.code),
                              'y', (SELECT y.label FROM tb_y y WHERE y.code = t.code)) AS data
    FROM tb_thing t $$, '{"uncascaded_policy": "warn"}');

CREATE FUNCTION assert_fresh(step text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT pk_thing, data FROM tv_thing EXCEPT SELECT pk_thing, data FROM tviews.public__tv_thing)
       OR EXISTS (SELECT pk_thing, data FROM tviews.public__tv_thing EXCEPT SELECT pk_thing, data FROM tv_thing) THEN
        RAISE EXCEPTION 'transition plans FAIL after %: %', step,
            (SELECT jsonb_agg(data ORDER BY pk_thing) FROM tv_thing);
    END IF;
END $$;

-- Alternate writes to both tables in one session, several times each, so a plan
-- kept for one would be reused for the other.
DO $$
DECLARE i int;
BEGIN
    FOR i IN 1..3 LOOP
        UPDATE tb_x SET label = 'x' || id || '.' || i WHERE id <= 3;
        UPDATE tb_y SET label = 'y' || id || '.' || i WHERE id <= 3;
        UPDATE tb_x SET code = 'c' || (7 - id) WHERE id IN (1, 6);
        UPDATE tb_y SET code = 'c' || (7 - id) WHERE id IN (1, 6);
    END LOOP;
END $$;
SELECT assert_fresh('alternating writes');
DELETE FROM tb_y WHERE id = 2;
DELETE FROM tb_x WHERE id = 3;
INSERT INTO tb_y VALUES (9, 'n', 'y-new', 'c2', 20);
INSERT INTO tb_x VALUES (20, 'c3', 'x-new');
SELECT assert_fresh('deletes and inserts');

SELECT 'transition plans per table: PASS' AS result;
-- expect-output: transition plans per table: PASS

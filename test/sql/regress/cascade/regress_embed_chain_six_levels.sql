-- A chain of six TVIEWs, each embedding the one before it (tv_level_N joins
-- tv_level_{N-1}), created through CREATE TABLE ... AS.
--
-- A root update reaches the last level (five embeds deep), twice in a row; a
-- mid-level update reaches the levels below it and leaves those above alone.
-- (A cascade that loops back is refused: regress_flush_errors_fail_writes.sql.)
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_embed_chain_six_levels.sql
--
-- expect-output: embed chain six levels: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'embed chain FAIL: %', what; END IF; END $$;
CREATE FUNCTION assert_levels_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    FOR i IN 0..5 LOOP
        PERFORM assert_fresh(format('tv_level_%s', i)::regclass, format('pk_level_%s', i), label);
    END LOOP;
END $$;

CREATE TABLE tb_level_0 (
    pk_level_0 INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    value TEXT NOT NULL
);
DO $$ BEGIN
    FOR i IN 1..5 LOOP
        EXECUTE format($f$
            CREATE TABLE tb_level_%1$s (
                pk_level_%1$s INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
                fk_level_%2$s INTEGER NOT NULL REFERENCES tb_level_%2$s(pk_level_%2$s),
                value TEXT NOT NULL)$f$, i, i - 1);
    END LOOP;
END $$;
INSERT INTO tb_level_0 (value) VALUES ('Root');
INSERT INTO tb_level_1 (fk_level_0, value) VALUES (1, 'Level 1');
INSERT INTO tb_level_2 (fk_level_1, value) VALUES (1, 'Level 2');
INSERT INTO tb_level_3 (fk_level_2, value) VALUES (1, 'Level 3');
INSERT INTO tb_level_4 (fk_level_3, value) VALUES (1, 'Level 4');
INSERT INTO tb_level_5 (fk_level_4, value) VALUES (1, 'Level 5');

CREATE TABLE tv_level_0 AS
SELECT pk_level_0, id, jsonb_build_object('id', id::text, 'value', value) AS data FROM tb_level_0;
CREATE TABLE tv_level_1 AS
SELECT l.pk_level_1, l.id, l.fk_level_0, p.id AS level_0_id,
       jsonb_build_object('id', l.id::text, 'value', l.value, 'parent', p.data) AS data
FROM tb_level_1 l JOIN tv_level_0 p ON p.pk_level_0 = l.fk_level_0;
CREATE TABLE tv_level_2 AS
SELECT l.pk_level_2, l.id, l.fk_level_1, p.id AS level_1_id,
       jsonb_build_object('id', l.id::text, 'value', l.value, 'parent', p.data) AS data
FROM tb_level_2 l JOIN tv_level_1 p ON p.pk_level_1 = l.fk_level_1;
CREATE TABLE tv_level_3 AS
SELECT l.pk_level_3, l.id, l.fk_level_2, p.id AS level_2_id,
       jsonb_build_object('id', l.id::text, 'value', l.value, 'parent', p.data) AS data
FROM tb_level_3 l JOIN tv_level_2 p ON p.pk_level_2 = l.fk_level_2;
CREATE TABLE tv_level_4 AS
SELECT l.pk_level_4, l.id, l.fk_level_3, p.id AS level_3_id,
       jsonb_build_object('id', l.id::text, 'value', l.value, 'parent', p.data) AS data
FROM tb_level_4 l JOIN tv_level_3 p ON p.pk_level_3 = l.fk_level_3;
CREATE TABLE tv_level_5 AS
SELECT l.pk_level_5, l.id, l.fk_level_4, p.id AS level_4_id,
       jsonb_build_object('id', l.id::text, 'value', l.value, 'parent', p.data) AS data
FROM tb_level_5 l JOIN tv_level_4 p ON p.pk_level_4 = l.fk_level_4;

-- 1. All six registered and fresh.
SELECT assert_levels_fresh('creating the chain');
SELECT must((SELECT count(*) FROM pg_tview_meta) = 6, 'expected 6 TVIEWs in the chain');

-- 2. A root update reaches level 5.
UPDATE tb_level_0 SET value = 'Root Updated' WHERE pk_level_0 = 1;
SELECT assert_levels_fresh('an UPDATE of the root');
SELECT must((SELECT data #>> '{parent,parent,parent,parent,parent,value}' FROM tv_level_5)
            = 'Root Updated', 'root update did not reach level 5');

-- 3. A mid-level update reaches the levels below, not those above.
UPDATE tb_level_2 SET value = 'Level 2 Updated' WHERE pk_level_2 = 1;
SELECT assert_levels_fresh('an UPDATE of level 2');
SELECT must((SELECT data->>'value' FROM tv_level_2) = 'Level 2 Updated'
            AND (SELECT data #>> '{parent,parent,parent,value}' FROM tv_level_5) = 'Level 2 Updated',
            'level 2 update did not reach levels 2..5');
SELECT must((SELECT data #>> '{parent,value}' FROM tv_level_1) = 'Root Updated',
            'level 1 changed by a level 2 update');

-- 4. A second root update reaches level 5 too.
UPDATE tb_level_0 SET value = 'Root Again' WHERE pk_level_0 = 1;
SELECT assert_levels_fresh('a second UPDATE of the root');
SELECT must((SELECT data #>> '{parent,parent,parent,parent,parent,value}' FROM tv_level_5)
            = 'Root Again', 'second root update did not reach level 5');

\echo 'embed chain six levels: PASS'

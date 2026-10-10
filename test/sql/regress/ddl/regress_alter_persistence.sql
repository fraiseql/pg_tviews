-- ALTER TABLE tv_x SET LOGGED / SET UNLOGGED on a TVIEW keeps its rows and its
-- refresh.
--
-- A TVIEW is LOGGED unless its options say `logged: false` (ADR 0220); an
-- operator may switch it later with plain ALTER TABLE (PostgreSQL rewrites the
-- table). The rows must survive both ways, tviews.registry must report the new
-- persistence, and writes to the base table must keep refreshing the rewritten
-- table.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_alter_persistence.sql
-- expect-output: alter_persistence: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'alter_persistence FAIL: %', what; END IF; END $$;
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'alice'), (2, 'bob'), (3, 'carol');

SELECT pg_tviews_create('tv_item', $$
    SELECT pk_item, id, jsonb_build_object('name', name) AS data FROM tb_item $$);
CREATE FUNCTION persistence_is(want "char") RETURNS boolean LANGUAGE sql AS $$
    SELECT (SELECT relpersistence FROM pg_class WHERE oid = 'tv_item'::regclass) = want
       AND (SELECT logged FROM tviews.registry WHERE entity = 'item') = (want = 'p') $$;
SELECT must(persistence_is('p'), 'a TVIEW is LOGGED by default');

-- LOGGED -> UNLOGGED
ALTER TABLE tv_item SET UNLOGGED;
SELECT must(persistence_is('u'), 'SET UNLOGGED not reflected in pg_class / tviews.registry');
SELECT must((SELECT count(*) FROM tv_item) = 3, 'rows lost by SET UNLOGGED');
SELECT assert_fresh('tv_item', 'pk_item', 'SET UNLOGGED');
UPDATE tb_item SET name = 'alice 2' WHERE pk_item = 1;
INSERT INTO tb_item (pk_item, name) VALUES (4, 'dave');
SELECT assert_fresh('tv_item', 'pk_item', 'writes after SET UNLOGGED');

-- UNLOGGED -> LOGGED
ALTER TABLE tv_item SET LOGGED;
SELECT must(persistence_is('p'), 'SET LOGGED not reflected in pg_class / tviews.registry');
SELECT must((SELECT count(*) FROM tv_item) = 4, 'rows lost by SET LOGGED');
SELECT assert_fresh('tv_item', 'pk_item', 'SET LOGGED');
DELETE FROM tb_item WHERE pk_item = 2;
UPDATE tb_item SET name = 'carol 2' WHERE pk_item = 3;
SELECT assert_fresh('tv_item', 'pk_item', 'writes after SET LOGGED');

\echo 'alter_persistence: PASS'

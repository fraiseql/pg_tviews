-- Regression test (#308): re-registering a TVIEW that reads a relation no
-- trigger watches (a materialized view, a foreign table, a sequence).
--
-- pg_tviews_create() installs no trigger on such a relation. Re-registration
-- (pg_tviews_reregister[_all](), so ALTER EXTENSION pg_tviews UPDATE) re-synced
-- every table the stored plan reads, the matview among them, and failed with
-- "relation ... cannot have triggers". It now follows the same rule as create,
-- and a REFRESH MATERIALIZED VIEW still refreshes the TVIEW afterwards.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_reregister_unwatched.sql
--
-- expect-output: #308 reregister unwatched: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP EXTENSION IF EXISTS file_fdw CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE EXTENSION file_fdw;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#308 FAIL: %', what; END IF; END $$;

CREATE TABLE tb_category (pk_category int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                          name text);
CREATE MATERIALIZED VIEW mv_category AS SELECT pk_category, name FROM tb_category;
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_category int REFERENCES tb_category, label text);
INSERT INTO tb_category VALUES (1, DEFAULT, 'a');
INSERT INTO tb_item VALUES (1, DEFAULT, 1, 'x'), (2, DEFAULT, 1, 'y');
REFRESH MATERIALIZED VIEW mv_category;

-- A matview read through a view, under full_refresh.
CREATE VIEW v_item_src AS
  SELECT i.pk_item, i.id, jsonb_build_object('label', i.label, 'category', c.name) AS data
  FROM tb_item i LEFT JOIN mv_category c ON c.pk_category = i.fk_category;
SELECT tviews.pg_tviews_create('tv_item', 'SELECT pk_item, id, data FROM v_item_src',
    options => '{"uncascaded_tables": {"mv_category": "full_refresh"}}');
-- The same matview read directly, under warn.
SELECT tviews.pg_tviews_create('tv_tag', $$
  SELECT i.pk_item AS pk_tag, i.id, jsonb_build_object('category', c.name) AS data
  FROM tb_item i JOIN mv_category c ON c.pk_category = i.fk_category $$,
    '{"uncascaded_policy": "warn"}');
-- A foreign table and a sequence, under warn.
CREATE SERVER regress_308 FOREIGN DATA WRAPPER file_fdw;
CREATE FOREIGN TABLE tb_remote (pk_remote bigint, name text)
    SERVER regress_308 OPTIONS (program 'echo 1,alpha', format 'csv');
CREATE SEQUENCE s_counter;
SELECT tviews.pg_tviews_create('tv_loc', $$
  SELECT i.pk_item AS pk_loc, i.id,
         jsonb_build_object('remote', r.name, 'last', (SELECT last_value FROM s_counter)) AS data
  FROM tb_item i LEFT JOIN tb_remote r ON r.pk_remote = i.pk_item $$,
    '{"uncascaded_policy": "warn"}');

-- 1. Every TVIEW re-registers.
CREATE TABLE reregistered AS SELECT * FROM tviews.pg_tviews_reregister_all();
SELECT must(NOT EXISTS (SELECT 1 FROM reregistered WHERE status <> 'reregistered'),
            'reregister_all: ' || (SELECT string_agg(entity || ': ' || status, '; ' ORDER BY entity)
                                   FROM reregistered));
SELECT tviews.pg_tviews_reregister('tv_item');

-- 2. No trigger on any of the unwatched relations, and the health check agrees.
SELECT must(NOT EXISTS (SELECT 1 FROM pg_trigger
                        WHERE tgrelid IN ('mv_category'::regclass, 'tb_remote'::regclass,
                                          's_counter'::regclass)),
            'a trigger on an unwatched relation');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_check_health()
                        WHERE component IN ('triggers', 'reregister') AND status <> 'OK'),
            'health: ' || (SELECT string_agg(component || '=' || message, '; ')
                           FROM tviews.pg_tviews_check_health()
                           WHERE component IN ('triggers', 'reregister') AND status <> 'OK'));

-- 3. After re-registration, a REFRESH still refreshes the full_refresh TVIEW, and
--    writes to the watched table still reach every TVIEW.
UPDATE tb_category SET name = 'b';
REFRESH MATERIALIZED VIEW mv_category;
SELECT assert_fresh('tv_item', 'pk_item', 'REFRESH after re-registration');
INSERT INTO tb_item VALUES (3, DEFAULT, 1, 'z');
SELECT assert_fresh('tv_item', 'pk_item', 'insert after re-registration');
-- tv_tag reads the matview under warn: the REFRESH leaves its rows as they were,
-- but the new item reaches it.
SELECT must(EXISTS (SELECT 1 FROM tv_tag WHERE pk_tag = 3 AND data->>'category' = 'b'),
            'insert after re-registration (matview read directly, warn)');
SELECT assert_fresh('tv_loc', 'pk_loc', 'insert after re-registration (foreign table)');

DROP EXTENSION pg_tviews CASCADE;
DROP EXTENSION file_fdw CASCADE;
SELECT '#308 reregister unwatched: PASS' AS result;

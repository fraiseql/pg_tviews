-- Regression test for issue #195: a per-table uncascaded_policy. A table named in
-- the `uncascaded_tables` option gets its own policy; every other table no cascade
-- reaches still falls under `uncascaded_policy`. A named table the definition
-- doesn't read, or whose writes are traced, is refused, so the list cannot rot.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/policy/regress_per_table_policy.sql
--
-- expect-output: issue #195 per-table policy: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#195 FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'created';
EXCEPTION WHEN OTHERS THEN RETURN SQLERRM; END $$;


CREATE SCHEMA catalog;
CREATE TABLE tb_locale (code text PRIMARY KEY, label text);
CREATE TABLE catalog.tb_currency (code text PRIMARY KEY, symbol text);
CREATE TABLE tb_extra (code text PRIMARY KEY, note text);
CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_locale VALUES ('fr', 'Français');
INSERT INTO catalog.tb_currency VALUES ('EUR', '€');
INSERT INTO tb_extra VALUES ('x', 'n');
INSERT INTO tb_item VALUES (1, default, 'a'), (2, default, 'b');
CREATE TABLE tb_item2 (LIKE tb_item);
ALTER TABLE tb_item2 RENAME pk_item TO pk_item2;
ALTER TABLE tb_item2 ADD PRIMARY KEY (pk_item2);
CREATE TABLE tb_item3 (LIKE tb_item2);
ALTER TABLE tb_item3 RENAME pk_item2 TO pk_item3;
ALTER TABLE tb_item3 ADD PRIMARY KEY (pk_item3);
INSERT INTO tb_item2 SELECT * FROM tb_item;
INSERT INTO tb_item3 SELECT * FROM tb_item;

\set def 'SELECT i.pk_item, i.id, i.name, (SELECT l.label FROM tb_locale l WHERE l.code = ''fr'') AS locale, (SELECT c.symbol FROM catalog.tb_currency c WHERE c.code = ''EUR'') AS currency FROM tb_item i'
\set def_extra 'SELECT i.pk_item, i.id, i.name, (SELECT l.label FROM tb_locale l WHERE l.code = ''fr'') AS locale, (SELECT c.symbol FROM catalog.tb_currency c WHERE c.code = ''EUR'') AS currency, (SELECT e.note FROM tb_extra e WHERE e.code = ''x'') AS extra FROM tb_item i'

-- 1. The issue's options: both reference tables refresh in full, the rest refuses.
SELECT must(tviews.pg_tviews_create_or_replace('public.tv_item', :'def', '{
  "uncascaded_policy": "error",
  "uncascaded_tables": {"public.tb_locale": "full_refresh", "catalog.tb_currency": "full_refresh"}
}') = 'created', 'the issue''s options');
SELECT must(uncascaded_policy = 'error'
            AND uncascaded_table_policies = '{"tb_locale": "full_refresh", "catalog.tb_currency": "full_refresh"}',
            'registry: ' || uncascaded_policy || ' ' || uncascaded_table_policies::text)
FROM tviews.registry WHERE entity = 'item';
UPDATE tb_locale SET label = 'French';
SELECT assert_fresh('tv_item', 'pk_item', 'a write to a full_refresh table');
UPDATE catalog.tb_currency SET symbol = 'EUR';
SELECT assert_fresh('tv_item', 'pk_item', 'a write to the other full_refresh table');
UPDATE tb_item SET name = 'A' WHERE pk_item = 1;
SELECT assert_fresh('tv_item', 'pk_item', 'a write to the root table');
-- The same options again: unchanged.
SELECT must(tviews.pg_tviews_create_or_replace('public.tv_item', :'def', '{
  "uncascaded_policy": "error",
  "uncascaded_tables": {"catalog.tb_currency": "full_refresh", "tb_locale": "full_refresh"}
}') = 'unchanged', 'the same options');

-- 2. A third untraced table, not named, is refused under the TVIEW's policy.
SELECT must(error_of(format($$SELECT tviews.pg_tviews_create_or_replace('public.tv_item', %L, '{
  "uncascaded_policy": "error",
  "uncascaded_tables": {"public.tb_locale": "full_refresh", "catalog.tb_currency": "full_refresh"}
}')$$, :'def_extra')) ~ '^writes to public\.tb_extra would not refresh',
            'an unnamed untraced table');

-- 3. Only the map changes: altered, in place.
SELECT must(tviews.pg_tviews_create_or_replace('public.tv_item', :'def', '{
  "uncascaded_policy": "warn",
  "uncascaded_tables": {"public.tb_locale": "full_refresh"}
}') = 'altered', 'the map changed');
SELECT must(uncascaded_policy = 'warn' AND uncascaded_table_policies = '{"tb_locale": "full_refresh"}',
            'registry after altered: ' || uncascaded_table_policies::text)
FROM tviews.registry WHERE entity = 'item';
UPDATE tb_locale SET label = 'Fr';
SELECT assert_fresh('tv_item', 'pk_item', 'full_refresh table after the map changed');
-- Omitted, the map is the default, none (the options are the whole declaration).
SELECT must(tviews.pg_tviews_create_or_replace('public.tv_item', :'def', '{"uncascaded_policy": "warn"}')
            = 'altered', 'an omitted map');
SELECT must(options->'uncascaded_tables' = '{}', 'an omitted map cleared: ' || options::text)
FROM tviews.registry WHERE entity = 'item';
SELECT tviews.pg_tviews_create_or_replace('public.tv_item', :'def', '{
  "uncascaded_policy": "warn",
  "uncascaded_tables": {"public.tb_locale": "full_refresh"}
}');

-- 4. A table named with "error" under a full_refresh TVIEW is refused alone.
SELECT must(outcome ~ '^writes to catalog\.tb_currency would not refresh', 'a table named with error: ' || outcome)
FROM (SELECT error_of($$SELECT tviews.pg_tviews_create_or_replace('public.tv_item2', $q$
    SELECT i.pk_item2, i.id, (SELECT l.label FROM tb_locale l WHERE l.code = 'fr') AS locale,
           (SELECT c.symbol FROM catalog.tb_currency c WHERE c.code = 'EUR') AS currency FROM tb_item2 i $q$, '{
  "uncascaded_policy": "full_refresh", "uncascaded_tables": {"catalog.tb_currency": "error"}}')$$) AS outcome) o;

-- 5. D3: a named table the definition doesn't read, or that is traced, is refused.
SELECT must(error_of(format($$SELECT tviews.pg_tviews_create_or_replace('public.tv_item', %L, '{
  "uncascaded_policy": "error",
  "uncascaded_tables": {"public.tb_locale": "full_refresh", "catalog.tb_currency": "full_refresh",
                        "public.tb_extra": "full_refresh"}}')$$, :'def'))
            LIKE '%tb_extra%not read%', 'a named table that is not read');
SELECT must(error_of(format($$SELECT tviews.pg_tviews_create_or_replace('public.tv_item', %L, '{
  "uncascaded_policy": "error",
  "uncascaded_tables": {"public.tb_locale": "full_refresh", "catalog.tb_currency": "full_refresh",
                        "public.tb_item": "full_refresh"}}')$$, :'def'))
            LIKE '%tb_item%traced%', 'a named table that is traced');
SELECT must(error_of(format($$SELECT tviews.pg_tviews_create_or_replace('public.tv_item', %L, '{
  "uncascaded_tables": {"public.tb_nowhere": "full_refresh"}}')$$, :'def')) LIKE '%tb_nowhere does not exist%',
            'a named table that does not exist');
SELECT must(error_of($$SELECT tviews.pg_tviews_create_or_replace('public.tv_item', 'SELECT 1', '{
  "uncascaded_tables": {"public.tb_locale": "static"}}')$$) LIKE '%uncascaded_tables%',
            'an unknown policy');
SELECT must(error_of($$SELECT tviews.pg_tviews_create_or_replace('public.tv_item', 'SELECT 1', '{
  "uncascaded_tables": ["public.tb_locale"]}')$$) LIKE '%uncascaded_tables%', 'not an object');

-- 6. A materialized view named full_refresh: REFRESH MATERIALIZED VIEW rebuilds.
CREATE MATERIALIZED VIEW mv_locale AS SELECT count(*) AS n FROM tb_locale;
SELECT tviews.pg_tviews_create_or_replace('public.tv_item3', $q$
    SELECT i.pk_item3, i.id, (SELECT n FROM mv_locale) AS locales FROM tb_item3 i $q$,
    '{"uncascaded_tables": {"mv_locale": "full_refresh"}}');
INSERT INTO tb_locale VALUES ('de', 'Deutsch');
REFRESH MATERIALIZED VIEW mv_locale;
SELECT assert_fresh('tv_item3', 'pk_item3', 'REFRESH of a materialized view named full_refresh');

-- 7. Re-registration keeps the map.
SELECT tviews.pg_tviews_reregister('item');
SELECT must(uncascaded_table_policies = '{"tb_locale": "full_refresh"}',
            'registry after reregister: ' || uncascaded_table_policies::text)
FROM tviews.registry WHERE entity = 'item';

\echo issue #195 per-table policy: PASS

-- Regression test (#133): a versioned read contract for tools.
--
-- confiture read pg_tview_meta, an internal table that changes between releases.
-- tviews.registry is the stable view over it, and tviews.contract_version() says
-- which contract it follows. Values come from the system catalogs where they can,
-- so the view reports the truth after a manual ALTER TABLE, and it is plain SQL.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_133_read_contract.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS app CASCADE;
DROP ROLE IF EXISTS regress_133_reader;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE SCHEMA app;
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE app.tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES tb_user,
    title   text
);
CREATE TABLE tb_order (
    pk_order int PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user  int NOT NULL REFERENCES tb_user,
    total    numeric
);
CREATE VIEW app.v_titles AS SELECT pk_post, upper(title) AS title FROM app.tb_post;
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice');
INSERT INTO app.tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
INSERT INTO tb_order (pk_order, fk_user, total) VALUES (1, 1, 10);

SET pg_tviews.unlogged_by_default = off;
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
-- Off-path, reading a plain view (followed), another TVIEW's view (followed) and
-- another TVIEW's table (listed, not followed).
SET search_path TO app, public, tviews;
SET pg_tviews.data_gin_index = on;
SET pg_tviews.unlogged_by_default = on;
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', t.title, 'author', u.data, 'name', tu.data->>'name') AS data
    FROM app.tb_post p
    JOIN app.v_titles t ON t.pk_post = p.pk_post
    JOIN tviews.public__tv_user u ON u.pk_user = p.fk_user
    JOIN public.tv_user tu ON tu.pk_user = p.fk_user $$);
RESET pg_tviews.data_gin_index;
RESET pg_tviews.unlogged_by_default;
RESET search_path;
SELECT pg_tviews_create_aggregate('tv_user_orders', $$
    SELECT o.fk_user AS pk_user_orders, u.id, jsonb_build_object('orders', count(*)) AS data
    FROM tb_order o JOIN tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id
$$, '{"tb_order": "fk_user", "tb_user": "pk_user"}');

-- A manual change the registry must see.
ALTER TABLE tv_user SET (fillfactor = 70);

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#133 FAIL: %', what; END IF; END $$;

-- 1. The contract version and the view's columns, in order.
SELECT must(tviews.contract_version() = 1, 'contract_version() is not 1');
SELECT must(
    (SELECT string_agg(attname || ' ' || format_type(atttypid, atttypmod), ', ' ORDER BY attnum)
     FROM pg_attribute WHERE attrelid = 'tviews.registry'::regclass AND attnum > 0)
    = 'schema text, name text, entity text, query text, base_tables regclass[], '
      'logged boolean, options jsonb, needs_reregister boolean, view regclass, '
      'uncascaded_tables regclass[], uncascaded_policy text, cascade_kinds jsonb, identity text[], '
      'uncascaded_table_policies jsonb, function_reads jsonb, time_dependent boolean, time_refresh text',
    'tviews.registry columns differ from contract 1');

-- 2. One row per TVIEW, with catalog truth.
SELECT must((SELECT count(*) FROM tviews.registry) = 3, 'expected three rows');
SELECT must(
    (SELECT schema = 'public' AND name = 'tv_user' AND logged
            AND options = '{"logged": true, "fillfactor": 70, "data_gin_index": false,
                            "group_keys": null}'::jsonb
     FROM tviews.registry WHERE entity = 'user'),
    'tv_user row: ' || (SELECT row(schema, name, logged, options)::text
                        FROM tviews.registry WHERE entity = 'user'));
SELECT must(
    (SELECT schema = 'app' AND name = 'tv_post' AND NOT logged
            AND options->'logged' = 'false' AND options->'data_gin_index' = 'true'
            AND options->'fillfactor' IS NOT NULL AND options->'group_keys' = 'null'
     FROM tviews.registry WHERE entity = 'post'),
    'tv_post row: ' || (SELECT row(schema, name, logged, options)::text
                        FROM tviews.registry WHERE entity = 'post'));
SELECT must(
    (SELECT options->'group_keys' FROM tviews.registry WHERE entity = 'user_orders')
    = '{"tb_order": "fk_user", "tb_user": "pk_user"}'::jsonb,
    'tv_user_orders group_keys');
SELECT must(
    (SELECT query FROM tviews.registry WHERE entity = 'user')
    = (SELECT definition FROM tviews.pg_tview_meta WHERE entity = 'user'),
    'query is not the stored definition');

-- 3. base_tables: views followed, another TVIEW's table listed, sorted by schema
--    then name.
SELECT must(
    (SELECT base_tables::text[] FROM tviews.registry WHERE entity = 'post')
    = ARRAY['app.tb_post', 'tb_user', 'tv_user'],
    'tv_post base_tables: ' || (SELECT base_tables::text FROM tviews.registry
                                WHERE entity = 'post'));
SELECT must(
    (SELECT base_tables::text[] FROM tviews.registry WHERE entity = 'user_orders')
    = ARRAY['tb_order', 'tb_user'],
    'tv_user_orders base_tables');

-- A user's jsonb_path_ops GIN index on data is not pg_tviews' data_gin_index.
CREATE INDEX tv_user_path_idx ON tv_user USING gin (data jsonb_path_ops);
SELECT must((SELECT NOT (options->>'data_gin_index')::boolean FROM tviews.registry
             WHERE entity = 'user'), 'jsonb_path_ops index counted as data_gin_index');
DROP INDEX tv_user_path_idx;

-- 4. needs_reregister follows the catalog.
UPDATE tviews.pg_tview_meta SET needs_reregister = true WHERE entity = 'user';
SELECT must((SELECT needs_reregister FROM tviews.registry WHERE entity = 'user'),
            'needs_reregister not reported');
UPDATE tviews.pg_tview_meta SET needs_reregister = false WHERE entity = 'user';

-- 5. Plain SQL: the view and contract_version() call no function of the library.
SELECT must(NOT EXISTS (
    SELECT 1 FROM pg_depend d
    JOIN pg_proc p ON p.oid = d.refobjid AND d.refclassid = 'pg_proc'::regclass
    WHERE d.classid = 'pg_rewrite'::regclass
      AND d.objid = (SELECT oid FROM pg_rewrite WHERE ev_class = 'tviews.registry'::regclass)
      AND p.prolang = (SELECT oid FROM pg_language WHERE lanname = 'c')),
    'tviews.registry calls a C function');
SELECT must((SELECT l.lanname FROM pg_proc p JOIN pg_language l ON l.oid = p.prolang
             WHERE p.oid = 'tviews.contract_version()'::regprocedure) = 'sql'
            AND (SELECT provolatile FROM pg_proc
                 WHERE oid = 'tviews.contract_version()'::regprocedure) = 's',
            'contract_version() is not a STABLE SQL function');

-- 6. Readable by any role.
CREATE ROLE regress_133_reader;
SET ROLE regress_133_reader;
SELECT must((SELECT count(*) FROM tviews.registry) = 3, 'a plain role cannot read the registry');
SELECT must(tviews.contract_version() = 1, 'a plain role cannot call contract_version()');
RESET ROLE;

DROP SCHEMA app CASCADE;
DROP EXTENSION pg_tviews CASCADE;
DROP ROLE regress_133_reader;

SELECT 'issue #133 read contract: PASS' AS result;
-- expect-output: issue #133 read contract: PASS

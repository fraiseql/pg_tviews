-- tviews.registry.managed_indexes lists the indexes pg_tviews created on a TVIEW's
-- table, and pg_tviews removes or leaves out only those (#218, #219).
--
-- A tool reads a TVIEW's user indexes as: every index on the table, minus
-- managed_indexes, minus constraint-backed indexes. A replace carries user indexes
-- over and never drops one; turning data_gin_index off drops only pg_tviews' GIN
-- index, and options.data_gin_index reports that index, not a user's.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_managed_indexes.sql
-- expect-output: managed_indexes: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
SET check_function_bodies = off;

DROP ROLE IF EXISTS regress_219_migrator;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'managed_indexes FAIL: %', what; END IF; END $$;
CREATE FUNCTION cor(name text, query text, options jsonb DEFAULT '{}') RETURNS text
LANGUAGE sql AS $$ SELECT tviews.pg_tviews_create_or_replace(name, query, options) $$;
-- The indexes on `tv` that pg_tviews manages, and those it does not, by name.
CREATE FUNCTION managed(tv regclass) RETURNS text[] LANGUAGE sql AS $$
    SELECT managed_indexes::text[] FROM tviews.registry
    WHERE format('%I.%I', schema, name)::regclass = tv $$;
CREATE FUNCTION unmanaged(tv regclass) RETURNS text[] LANGUAGE sql AS $$
    SELECT COALESCE(array_agg(i.indexrelid::regclass::text ORDER BY 1), '{}')
    FROM pg_index i
    WHERE i.indrelid = tv
      AND NOT EXISTS (SELECT 1 FROM pg_constraint k WHERE k.conindid = i.indexrelid)
      AND i.indexrelid <> ALL (COALESCE((SELECT managed_indexes FROM tviews.registry r
                                         WHERE format('%I.%I', r.schema, r.name)::regclass = tv),
                                        '{}')::oid[]) $$;
CREATE FUNCTION gin_option(entity_name text) RETURNS boolean LANGUAGE sql AS $$
    SELECT (options->>'data_gin_index')::boolean FROM tviews.registry WHERE entity = entity_name $$;

-- A migration role, not a superuser, owns the schema and the TVIEWs.
CREATE ROLE regress_219_migrator;
CREATE SCHEMA app AUTHORIZATION regress_219_migrator;
GRANT USAGE ON SCHEMA tviews TO regress_219_migrator;
SET ROLE regress_219_migrator;
SET search_path TO app, public, tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      author_pk bigint REFERENCES tb_user, title text);
CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO tb_post (pk_post, author_pk, title) VALUES (1, 1, 'one'), (2, 2, 'two');
INSERT INTO tb_item (pk_item, name) VALUES (1, 'a'), (2, 'b');

-- 1. Behaviour first: #218.
SELECT cor('app.tv_item', $$SELECT pk_item, id, jsonb_build_object('name', name) AS data
                             FROM tb_item$$, '{"data_gin_index": true}');
-- A user's GIN on data under another name survives turning the option off.
CREATE INDEX my_item_gin ON tv_item USING gin (data jsonb_path_ops);
SELECT must(cor('app.tv_item', $$SELECT pk_item, id, jsonb_build_object('name', name) AS data
                                 FROM tb_item$$, '{"data_gin_index": false}') = 'altered',
            'toggle off is an altered replace');
SELECT must(to_regclass('app.my_item_gin') IS NOT NULL, '#218: toggle off dropped my_item_gin');
SELECT must(to_regclass('app.idx_tv_item_data_gin') IS NULL, 'toggle off kept the managed GIN');

-- A user's default-opclass GIN on data is not pg_tviews' option.
CREATE INDEX my_default_gin ON tv_item USING gin (data);
SELECT must(gin_option('item') = false, '#218: a user GIN reads as data_gin_index');
SELECT must(cor('app.tv_item', $$SELECT pk_item, id, jsonb_build_object('name', name) AS data
                                 FROM tb_item$$, '{"data_gin_index": true}') = 'altered',
            'toggle on with a user GIN present is an altered replace');
SELECT must(to_regclass('app.idx_tv_item_data_gin') IS NOT NULL, 'toggle on did not create the GIN');
SELECT must(gin_option('item'), 'data_gin_index off after toggle on');

-- A rebuilt replace carries every user index over and records its own anew.
SELECT must(cor('app.tv_item', $$SELECT pk_item, id, name, jsonb_build_object('name', name) AS data
                                 FROM tb_item$$, '{"data_gin_index": true}') = 'rebuilt',
            'a new column rebuilds');
SELECT must(unmanaged('app.tv_item') = '{my_default_gin,my_item_gin}',
            'rebuild lost a user index: ' || unmanaged('app.tv_item')::text);
SELECT must(managed('app.tv_item') = '{idx_tv_item_data_gin,idx_tv_item_id}',
            'managed after rebuild: ' || managed('app.tv_item')::text);
SELECT assert_fresh('tv_item', 'pk_item', 'after rebuild');

-- 2. managed_indexes: sorted by name, the primary key left out.
SELECT cor('app.tv_user', $$SELECT pk_user, id, jsonb_build_object('name', name) AS data
                            FROM tb_user$$);
SELECT must(managed('app.tv_user') = '{idx_tv_user_id}', 'tv_user: ' || managed('app.tv_user')::text);
-- An embed lookup column gets its propagation index, recorded.
SELECT cor('app.tv_post', $$SELECT p.pk_post, p.id, p.author_pk,
                                   jsonb_build_object('title', p.title, 'author', u.data) AS data
                            FROM tb_post p JOIN tv_user u ON u.pk_user = p.author_pk$$);
SELECT must(managed('app.tv_post') = '{idx_tv_post_author_pk_pk_post,idx_tv_post_id}',
            'tv_post: ' || managed('app.tv_post')::text);
-- A user's index is never listed; re-registration lists nothing twice.
CREATE INDEX post_by_author ON tv_post (author_pk);
SELECT tviews.pg_tviews_reregister('post');
SELECT must(managed('app.tv_post') = '{idx_tv_post_author_pk_pk_post,idx_tv_post_id}',
            'after reregister: ' || managed('app.tv_post')::text);
SELECT must(unmanaged('app.tv_post') = '{post_by_author}', 'tv_post user indexes');

-- pg_tviews_ensure_propagation_indexes() records what it creates.
DROP INDEX idx_tv_post_author_pk_pk_post, post_by_author;
SELECT must((SELECT count(*) FROM tviews.pg_tviews_ensure_propagation_indexes('post')) = 1,
            'ensure_propagation_indexes created nothing');
SELECT must(managed('app.tv_post') = '{idx_tv_post_author_pk_pk_post,idx_tv_post_id}',
            'after ensure: ' || managed('app.tv_post')::text);

-- Writes keep refreshing.
UPDATE tb_user SET name = 'alice 2' WHERE pk_user = 1;
INSERT INTO tb_item (pk_item, name) VALUES (3, 'c');
SELECT assert_fresh('tv_post', 'pk_post', 'writes');
SELECT assert_fresh('tv_item', 'pk_item', 'writes');

-- 3. Catalog shape: an appended regclass[]; NULL when the table is gone.
RESET ROLE;
SELECT must((SELECT attname = 'managed_indexes' AND atttypid = 'regclass[]'::regtype
             FROM pg_attribute WHERE attrelid = 'tviews.registry'::regclass AND attnum > 0
             ORDER BY attnum DESC LIMIT 1), 'managed_indexes is not the last column');
SELECT must((SELECT array_length(managed_index_names, 1) FROM tviews.pg_tview_meta
             WHERE entity = 'user') = 1, 'pg_tview_meta.managed_index_names');
UPDATE tviews.pg_tview_meta SET table_oid = 4000000000 WHERE entity = 'item';
SELECT must((SELECT managed_indexes IS NULL FROM tviews.registry WHERE entity = 'item'),
            'managed_indexes of a registration whose table is gone');

DROP SCHEMA app CASCADE;
DROP EXTENSION pg_tviews CASCADE;
DROP OWNED BY regress_219_migrator;
DROP ROLE regress_219_migrator;

SELECT 'managed_indexes: PASS';

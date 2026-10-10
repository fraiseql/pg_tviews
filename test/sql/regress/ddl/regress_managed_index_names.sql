-- The index names pg_tviews manages on a TVIEW's table stay its own (#219).
--
-- A user's ALTER INDEX … RENAME and DROP INDEX of a managed index are followed:
-- the renamed index stays managed, a dropped one is forgotten. A user's index may
-- not take a name pg_tviews manages or would create for the TVIEW, so a managed
-- CREATE INDEX IF NOT EXISTS never finds a user's index in its place. The
-- statements pg_tviews_ensure_propagation_indexes() reports for running by hand
-- (with CONCURRENTLY) are the exception: accepted, and recorded as managed.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_managed_index_names.sql
-- expect-output: managed_index_names: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
SET check_function_bodies = off;

DROP ROLE IF EXISTS regress_219_owner;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'managed_index_names FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ': ' || SQLERRM; END $$;
CREATE FUNCTION recorded(entity_name text) RETURNS text[] LANGUAGE sql AS $$
    SELECT ARRAY(SELECT n FROM tviews.pg_tview_meta m, unnest(m.managed_index_names) n
                 WHERE m.entity = entity_name ORDER BY n) $$;
CREATE FUNCTION managed(entity_name text) RETURNS text[] LANGUAGE sql AS $$
    SELECT managed_indexes::text[] FROM tviews.registry WHERE entity = entity_name $$;

CREATE ROLE regress_219_owner;
CREATE SCHEMA app AUTHORIZATION regress_219_owner;
GRANT USAGE ON SCHEMA tviews TO regress_219_owner;
SET ROLE regress_219_owner;
SET search_path TO app, public, tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      author_pk bigint REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO tb_post (pk_post, author_pk, title) VALUES (1, 1, 'one'), (2, 2, 'two');
SELECT tviews.pg_tviews_create_or_replace('app.tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT p.pk_post, p.id, p.author_pk, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.author_pk $$);

-- 1. A user's index may not take a name pg_tviews would create: the managed GIN of
--    a TVIEW without it, a dropped managed index.
SELECT must(error_of($$CREATE INDEX idx_tv_post_data_gin ON tv_post USING gin (data jsonb_path_ops)$$)
            LIKE '42939: %idx_tv_post_data_gin%', 'user index took the managed GIN name');
SELECT must(to_regclass('app.idx_tv_post_data_gin') IS NULL, 'refused index exists');
-- Another name on the same table, and the same name on a plain table, are fine.
CREATE INDEX post_data_gin ON tv_post USING gin (data jsonb_path_ops);
CREATE INDEX idx_tv_post_data_gin ON tb_post (title);
DROP INDEX idx_tv_post_data_gin;

-- 2. DROP INDEX of a managed index is followed; its name stays reserved.
DROP INDEX idx_tv_post_author_pk_pk_post;
SELECT must(recorded('post') = '{idx_tv_post_id}', 'drop not followed: ' || recorded('post')::text);
SELECT must(error_of($$CREATE INDEX idx_tv_post_author_pk_pk_post ON tv_post (author_pk)$$)
            LIKE '42939: %', 'user index took a dropped managed name');
SELECT must(error_of($$ALTER INDEX post_data_gin RENAME TO idx_tv_post_author_pk_pk_post$$)
            LIKE '42939: %', 'rename into a managed name');

-- 3. What pg_tviews_ensure_propagation_indexes() reports, run by hand, is accepted
--    and recorded.
SELECT must((SELECT array_agg(s) FROM tviews.pg_tviews_ensure_propagation_indexes('post', true) s)
            = ARRAY['CREATE INDEX IF NOT EXISTS idx_tv_post_author_pk_pk_post '
                    'ON app.tv_post (author_pk, pk_post)'],
            'dry run: ' || (SELECT string_agg(s, '; ')
                            FROM tviews.pg_tviews_ensure_propagation_indexes('post', true) s));
CREATE INDEX CONCURRENTLY idx_tv_post_author_pk_pk_post ON app.tv_post (author_pk, pk_post);
SELECT must(managed('post') = '{idx_tv_post_author_pk_pk_post,idx_tv_post_id}',
            'hand-run propagation index not recorded: ' || managed('post')::text);

-- 4. ALTER INDEX … RENAME (and ALTER TABLE … RENAME on an index) is followed.
ALTER INDEX idx_tv_post_id RENAME TO post_by_id;
ALTER TABLE idx_tv_post_author_pk_pk_post RENAME TO post_by_author;
SELECT must(managed('post') = '{post_by_author,post_by_id}', 'rename not followed: ' || managed('post')::text);
-- A rebuild does not carry a renamed managed index over next to its new one.
SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT p.pk_post, p.id, p.author_pk, p.title,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.author_pk $$);
SELECT must((SELECT count(*) FROM pg_index WHERE indrelid = 'tv_post'::regclass
             AND pg_get_indexdef(indexrelid) LIKE '%USING btree (id)') = 1, 'id indexed twice');
SELECT must(managed('post') = '{idx_tv_post_author_pk_pk_post,idx_tv_post_id}',
            'after rebuild: ' || managed('post')::text);
SELECT must(to_regclass('app.post_data_gin') IS NOT NULL, 'user index lost by the rebuild');

UPDATE tb_user SET name = 'alice 2' WHERE pk_user = 1;
SELECT assert_fresh('tv_post', 'pk_post', 'writes');

RESET ROLE;
DROP SCHEMA app CASCADE;
DROP EXTENSION pg_tviews CASCADE;
DROP OWNED BY regress_219_owner;
DROP ROLE regress_219_owner;

SELECT 'managed_index_names: PASS';

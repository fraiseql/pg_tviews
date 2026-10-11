-- Regression test (#134): pg_tviews_create_or_replace(), the one way to create.
--
-- pg_tviews_create() failed when the TVIEW existed, so a migration using it could
-- not be re-applied, and took no storage options. create_or_replace() compares the
-- definition and options with what exists and makes the smallest change: created,
-- unchanged, altered (storage only, rows kept) or rebuilt (with the table's owner,
-- privileges, comment, GraphQL type name and user indexes carried over). It runs
-- the DDL as the caller and requires owning an existing TVIEW.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_create_or_replace.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS app CASCADE;
DROP SCHEMA IF EXISTS other CASCADE;
DROP SCHEMA IF EXISTS "Odd.Schema" CASCADE;
DROP ROLE IF EXISTS regress_134_migrator;
DROP ROLE IF EXISTS regress_134_reader;
DROP ROLE IF EXISTS regress_134_stranger;
-- In the same batch as CREATE EXTENSION.
CREATE EXTENSION jsonb_delta \; CREATE EXTENSION pg_tviews \; SELECT tviews.contract_version();

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#134 FAIL: %', what; END IF; END $$;
-- The error a statement raises, or NULL.
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ': ' || SQLERRM; END $$;
CREATE FUNCTION cor(name text, query text, options jsonb DEFAULT '{}') RETURNS text
LANGUAGE sql AS $$ SELECT tviews.pg_tviews_create_or_replace(name, query, options) $$;

-- A migration role: owns schema app, may read and trigger on its tables, no superuser.
CREATE ROLE regress_134_migrator;
CREATE ROLE regress_134_reader;
CREATE ROLE regress_134_stranger;
CREATE SCHEMA app AUTHORIZATION regress_134_migrator;
CREATE SCHEMA other AUTHORIZATION regress_134_migrator;
CREATE SCHEMA "Odd.Schema" AUTHORIZATION regress_134_migrator;
GRANT USAGE ON SCHEMA app, other TO regress_134_reader, regress_134_stranger;
GRANT EXECUTE ON FUNCTION must(boolean, text), error_of(text), cor(text, text, jsonb) TO PUBLIC;

SET ROLE regress_134_migrator;
SET search_path TO app, public, tviews;
CREATE TABLE app.tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE app.tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES app.tb_user,
    title   text,
    body    text
);
CREATE TABLE app.tb_order (
    pk_order int PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user  int NOT NULL REFERENCES app.tb_user,
    total    numeric
);
CREATE TABLE app.tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), label text);
CREATE TABLE app.tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), body text);
INSERT INTO app.tb_note (pk_note, body) VALUES (1, 'n1');
INSERT INTO app.tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO app.tb_post (pk_post, fk_user, title, body) VALUES (1, 1, 'p1', 'b1'), (2, 2, 'p2', 'b2');
INSERT INTO app.tb_order (pk_order, fk_user, total) VALUES (1, 1, 10), (2, 1, 5);
INSERT INTO app.tb_tag (pk_tag, label) VALUES (1, 't1'), (2, 't2');

-- 1. created, with storage options; unqualified names resolve to current_schema().
SELECT must(cor('app.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title) AS data
    FROM app.tb_post p $$, '{"logged": true, "fillfactor": 85}') = 'created', 'create tv_post');
SELECT must(cor('user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM app.tb_user $$)
    = 'created', 'create tv_user by entity name');
SELECT must((SELECT schema = 'app' AND logged AND (options->>'fillfactor')::int = 85
             FROM tviews.registry WHERE entity = 'post'), 'tv_post storage on create');
SELECT must((SELECT count(*) FROM app.tv_post) = 2, 'tv_post populated');
SELECT must(pg_get_userbyid((SELECT relowner FROM pg_class WHERE oid = 'app.tv_post'::regclass))
            = 'regress_134_migrator', 'tv_post owned by the caller');

-- 2. unchanged: same text, and text that differs only in layout, comments and case.
SELECT must(cor('app.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title) AS data
    FROM app.tb_post p $$) = 'unchanged', 'same definition');
SELECT must(cor('tv_post', $$select p.pk_post,p.id,p.fk_user,
    JSONB_BUILD_OBJECT('title', p.title) AS data -- a comment
    from app.tb_post AS p$$) = 'unchanged', 'same definition, other layout');
-- The options passed are the whole declaration: an omitted option is at its
-- default (ADR 0220), so a fillfactor tuned by hand is put back.
ALTER TABLE app.tv_post SET (fillfactor = 70);
SELECT must(cor('app.tv_post', (SELECT query FROM tviews.registry WHERE entity = 'post'))
            = 'altered', 'an omitted option is at its default');
SELECT must((SELECT (options->>'fillfactor')::int FROM tviews.registry WHERE entity = 'post') = 85,
            'the default fillfactor put back');
-- What the registry publishes recreates the TVIEW as it is (read contract).
SELECT must(cor(format('%I.%I', schema, name), query, options) = 'unchanged',
            'registry round trip: ' || options::text)
FROM tviews.registry WHERE entity = 'post';

-- 3. altered: storage only, rows kept.
CREATE TEMP TABLE post_rows AS SELECT pk_post, data, created_at FROM app.tv_post;
SELECT must(cor('app.tv_post', (SELECT query FROM tviews.registry WHERE entity = 'post'),
                '{"logged": false, "fillfactor": 60, "data_gin_index": true}') = 'altered',
            'storage change');
SELECT must((SELECT NOT logged AND (options->>'fillfactor')::int = 60
                    AND (options->>'data_gin_index')::boolean
             FROM tviews.registry WHERE entity = 'post'), 'altered storage');
SELECT must(NOT EXISTS (SELECT pk_post, data, created_at FROM app.tv_post
                        EXCEPT SELECT * FROM post_rows), 'rows kept by altered');
SELECT must(cor('app.tv_post', (SELECT query FROM tviews.registry WHERE entity = 'post'),
                '{"data_gin_index": false, "fillfactor": 100}') = 'altered', 'drop GIN index');
SELECT must((SELECT NOT (options->>'data_gin_index')::boolean
                    AND (options->>'fillfactor')::int = 100
             FROM tviews.registry WHERE entity = 'post'), 'GIN index dropped, fillfactor reset');

-- 4. rebuilt: new columns. Owner, privileges, comment and user indexes are
--    carried over; the options are those declared.
GRANT SELECT ON app.tv_post TO regress_134_reader;
COMMENT ON TABLE app.tv_post IS 'posts for the API';
CREATE INDEX tv_post_title_idx ON app.tv_post ((data->>'title'));
SELECT must(cor('app.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, p.title,
           jsonb_build_object('title', p.title, 'body', p.body) AS data
    FROM app.tb_post p $$, '{"logged": false, "typename": "BlogPost"}') = 'rebuilt',
            'column change rebuilds');
SELECT must(has_table_privilege('regress_134_reader', 'app.tv_post', 'SELECT'), 'privilege kept');
SELECT must(obj_description('app.tv_post'::regclass, 'pg_class') = 'posts for the API', 'comment kept');
SELECT must((SELECT options->>'typename' FROM tviews.registry WHERE entity = 'post') = 'BlogPost',
            'declared typename');
SELECT must(to_regclass('app.tv_post_title_idx') IS NOT NULL, 'user index kept');
SELECT must((SELECT NOT logged FROM tviews.registry WHERE entity = 'post'),
            'declared logged across a rebuild');
SELECT must((SELECT data->>'body' FROM app.tv_post WHERE pk_post = 1) = 'b1', 'rebuilt rows');
UPDATE app.tb_post SET body = 'b1!' WHERE pk_post = 1;
SELECT must((SELECT data->>'body' FROM app.tv_post WHERE pk_post = 1) = 'b1!',
            'rebuilt TVIEW follows its base table');

-- A user index that no longer applies fails the call, naming it; nothing changes.
CREATE INDEX tv_post_plain_title_idx ON app.tv_post (title);
SELECT must(error_of($x$SELECT cor('app.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title) AS data
    FROM app.tb_post p $$)$x$) LIKE '%tv_post_plain_title_idx%', 'stale user index named');
SELECT must((SELECT count(*) FROM information_schema.columns
             WHERE table_schema = 'app' AND table_name = 'tv_post' AND column_name = 'title') = 1,
            'failed rebuild rolled back');
DROP INDEX app.tv_post_plain_title_idx;

-- 5. rebuilt is refused when something depends on the TVIEW or cannot be carried.
CREATE VIEW app.post_titles AS SELECT pk_post, data->>'title' AS title FROM app.tv_post;
SELECT must(error_of($x$SELECT cor('app.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('t', p.title) AS data
    FROM app.tb_post p $$)$x$) LIKE '%post_titles%', 'dependent view named');
DROP VIEW app.post_titles;
ALTER TABLE app.tv_post ENABLE ROW LEVEL SECURITY;
SELECT must(error_of($x$SELECT cor('app.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('t', p.title) AS data
    FROM app.tb_post p $$)$x$) LIKE '%row level security%', 'RLS refused');
ALTER TABLE app.tv_post DISABLE ROW LEVEL SECURITY;
ALTER TABLE app.tv_post ADD CONSTRAINT tv_post_pk_positive CHECK (pk_post > 0);
SELECT must(error_of($x$SELECT cor('app.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('t', p.title) AS data
    FROM app.tb_post p $$)$x$) LIKE '%constraint tv_post_pk_positive%', 'user constraint refused');
ALTER TABLE app.tv_post DROP CONSTRAINT tv_post_pk_positive;
COMMENT ON COLUMN app.tv_post.data IS 'the post';
SELECT must(error_of($x$SELECT cor('app.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('t', p.title) AS data
    FROM app.tb_post p $$)$x$) LIKE '%comment on column data%', 'column comment refused');
COMMENT ON COLUMN app.tv_post.data IS NULL;

-- 6. Aggregate TVIEWs: created with group_keys, round trip, and group_keys: null
--    turns one into a plain TVIEW (a rebuild).
SELECT must(cor('app.tv_user_orders', $$
    SELECT o.fk_user AS pk_user_orders, u.id, jsonb_build_object('orders', count(*)) AS data
    FROM app.tb_order o JOIN app.tb_user u ON u.pk_user = o.fk_user
    GROUP BY o.fk_user, u.id $$, '{"group_keys": {"tb_order": "fk_user", "tb_user": "pk_user"}}')
    = 'created', 'create aggregate');
SELECT must(cor('app.tv_user_orders', (SELECT query FROM tviews.registry WHERE entity = 'user_orders'),
                (SELECT jsonb_build_object('group_keys', options->'group_keys')
                 FROM tviews.registry WHERE entity = 'user_orders')) = 'unchanged',
            'aggregate round trip');

-- 7. Raw SELECT and SELECT * definitions round-trip through registry.query.
SELECT must(cor('app.tv_tag', $$ SELECT pk_tag AS pk, id, label FROM app.tb_tag $$) = 'created',
            'raw select');
SELECT must(cor('app.tv_tag', (SELECT query FROM tviews.registry WHERE entity = 'tag'))
            = 'unchanged', 'raw select round trip');
CREATE VIEW app.v_note_src AS SELECT pk_note, id, jsonb_build_object('body', body) AS data
    FROM app.tb_note;
SELECT must(cor('app.tv_note', $$ SELECT * FROM app.v_note_src $$) = 'created', 'SELECT *');
SELECT must(cor('app.tv_note', (SELECT query FROM tviews.registry WHERE entity = 'note'))
            = 'unchanged', 'SELECT * round trip');

-- 8. Errors: options, names, definitions. A failing definition is raised, not read
--    as a change.
SELECT must(error_of($$SELECT cor('app.tv_post', 'SELECT 1', '{"colour": "red"}')$$)
            LIKE '%unknown option%colour%', 'unknown option');
SELECT must(error_of($$SELECT cor('app.tv_post', 'SELECT 1', '{"fillfactor": "85"}')$$)
            LIKE '%fillfactor%integer%', 'wrongly typed option');
SELECT must(error_of($$SELECT cor('app.tv_post', 'SELECT 1', '{"fillfactor": 5}')$$)
            LIKE '%fillfactor%10%100%', 'fillfactor out of range');
SELECT must(error_of($x$SELECT cor('app.tv_post', $$
    SELECT pk_user AS pk_article, id, jsonb_build_object() AS data FROM app.tb_user $$)$x$)
            LIKE '%pk_article%', 'name does not match the definition''s key');
SELECT must(error_of($x$SELECT cor('other.tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM app.tb_user $$)$x$)
            LIKE '%registered in schema app%', 'entity registered in another schema');
SELECT must(error_of($x$SELECT cor('app.tv_post', $$
    SELECT pk_post, id, fk_user, nope AS data FROM app.tb_post $$)$x$)
            LIKE '%nope%', 'invalid definition raised');
SELECT must((SELECT count(*) FROM app.tv_post) = 2, 'TVIEW intact after an invalid definition');

-- 9. Serialization: the call holds an advisory lock on the entity until commit.
BEGIN;
SELECT cor('app.tv_user', (SELECT query FROM tviews.registry WHERE entity = 'user'));
SELECT must(EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory'
                    AND pid = pg_backend_pid() AND granted), 'advisory lock held');
COMMIT;

-- 10. Anywhere: in a DO block and in an explicit transaction.
DO $$ BEGIN
    PERFORM must(tviews.pg_tviews_create_or_replace('app.tv_user',
        (SELECT query FROM tviews.registry WHERE entity = 'user')) = 'unchanged', 'DO block');
END $$;
RESET ROLE;

-- 11. Only the TVIEW's owner may replace or drop it.
SET ROLE regress_134_stranger;
SELECT must(error_of($$SELECT cor('app.tv_user', 'SELECT 1')$$) LIKE '42501:%',
            'stranger replaced tv_user');
SELECT must(error_of($$SELECT tviews.pg_tviews_drop('app.tv_user')$$) LIKE '42501:%',
            'stranger dropped tv_user');
RESET ROLE;

-- 12. pg_tviews_drop takes a qualified name.
SET ROLE regress_134_migrator;
SELECT tviews.pg_tviews_drop('app.tv_tag');
SELECT tviews.pg_tviews_drop('app.tv_tag', if_exists => true);
SELECT must(to_regclass('app.tv_tag') IS NULL
            AND NOT EXISTS (SELECT 1 FROM tviews.registry WHERE entity = 'tag'), 'qualified drop');
SELECT must(error_of($$SELECT tviews.pg_tviews_drop('other.tv_user')$$) IS NOT NULL,
            'drop in the wrong schema');

-- 13. A double-quoted part may contain dots.
CREATE TABLE "Odd.Schema".tb_odd (pk_odd int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), label text);
INSERT INTO "Odd.Schema".tb_odd (pk_odd, label) VALUES (1, 'o1');
SELECT must(cor('"Odd.Schema".tv_odd', $$
    SELECT pk_odd, id, jsonb_build_object('l', label) AS data FROM "Odd.Schema".tb_odd $$)
    = 'created', 'quoted schema created');
SELECT must((SELECT schema FROM tviews.registry WHERE entity = 'odd') = 'Odd.Schema',
            'quoted schema kept');
SELECT must(cor('"Odd.Schema".tv_odd', $$
    SELECT pk_odd, id, label, jsonb_build_object('l', label) AS data FROM "Odd.Schema".tb_odd $$)
    = 'rebuilt', 'quoted schema rebuilt');
SELECT tviews.pg_tviews_drop('"Odd.Schema".tv_odd');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.registry WHERE entity = 'odd'), 'quoted schema drop');
SELECT must(error_of($$SELECT cor('"app.tv_user', 'SELECT 1')$$) LIKE '%tview%',
            'unterminated quote');
RESET ROLE;

-- 14. A registration whose table is gone (dropped where the hook did not run) can
--     be dropped by the owner of what is left.
UPDATE tviews.pg_tview_meta SET table_oid = 4000000000 WHERE entity = 'note';
SET ROLE regress_134_stranger;
SELECT must(error_of($$SELECT tviews.pg_tviews_drop('app.tv_note')$$) LIKE '42501:%',
            'stranger dropped a stale registration');
SET ROLE regress_134_migrator;
SELECT tviews.pg_tviews_drop('app.tv_note');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.registry WHERE entity = 'note')
            AND to_regclass('app.v_note') IS NULL, 'stale registration dropped');
DROP TABLE app.tv_note;
RESET ROLE;

DROP SCHEMA app CASCADE;
DROP SCHEMA other CASCADE;
DROP SCHEMA "Odd.Schema" CASCADE;
DROP EXTENSION pg_tviews CASCADE;
DROP OWNED BY regress_134_migrator, regress_134_reader, regress_134_stranger;
DROP ROLE regress_134_migrator;
DROP ROLE regress_134_reader;
DROP ROLE regress_134_stranger;

SELECT 'issue #134 create_or_replace: PASS' AS result;
-- expect-output: issue #134 create_or_replace: PASS

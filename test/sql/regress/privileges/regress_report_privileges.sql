-- Regression test (#270): pg_tviews_flush_and_report() reports only the TVIEWs the
-- caller may read.
--
-- A writer's UPDATE on tb_user refreshes every TVIEW that embeds the user, as each
-- TVIEW's owner. The report then names what changed: without SELECT on a TVIEW the
-- caller learns nothing of its rows. Its type goes to `omitted_types`, as a type not
-- asked for does (#230), and its rows are neither listed nor read, so the call no
-- longer fails 42501. A collection is listed only when the caller may read both
-- the child and the parent TVIEW. The rule is table-level SELECT and USAGE on
-- the TVIEW's schema.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_report_privileges.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#270 FAIL: %', what; END IF; END $$;

-- How many entries of `report` have type `t`.
CREATE FUNCTION reported(report jsonb, t text) RETURNS bigint LANGUAGE sql AS $$
    SELECT count(*) FROM jsonb_array_elements((report->'updated') || (report->'deleted')) e
    WHERE e->>'__typename' = t $$;

-- User ← Post ← Comment; every document embeds its author's name.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                      name text NOT NULL);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text NOT NULL);
CREATE TABLE tb_comment (pk_comment int PRIMARY KEY,
                         id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                         fk_post int NOT NULL REFERENCES tb_post,
                         fk_user int NOT NULL REFERENCES tb_user, body text NOT NULL);
CREATE INDEX ON tb_post (fk_user);
CREATE INDEX ON tb_comment (fk_post);
CREATE INDEX ON tb_comment (fk_user);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 1, 'p2');
INSERT INTO tb_comment (pk_comment, fk_post, fk_user, body) VALUES (1, 1, 1, 'c1'), (2, 2, 1, 'c2');
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('id', u.id, 'name', u.name) AS data
    FROM tb_user u $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('id', p.id, 'title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
SELECT tviews.pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post, c.fk_user,
           jsonb_build_object('id', c.id, 'body', c.body, 'author', u.name) AS data
    FROM tb_comment c JOIN tb_user u ON u.pk_user = c.fk_user $$);

-- The writer may change users and comments, and read users and comments, not posts.
DROP ROLE IF EXISTS report_writer_270;
CREATE ROLE report_writer_270;
GRANT USAGE ON SCHEMA tviews TO report_writer_270;
GRANT SELECT, UPDATE ON tb_user, tb_comment TO report_writer_270;
GRANT SELECT ON tv_user, tv_comment TO report_writer_270;

-- 1. The default call (with data) succeeds and lists no post.
SET ROLE report_writer_270;
BEGIN;
UPDATE tb_user SET name = 'bob' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(reported(:'r', 'Post') = 0, 'with data, posts listed: ' || :'r');
SELECT must(reported(:'r', 'User') = 1 AND reported(:'r', 'Comment') = 2,
            'with data, readable rows missing: ' || :'r');
SELECT must((:'r'::jsonb)->'omitted_types' = '["Post"]', 'with data, omitted_types: ' || :'r');
SELECT must(NOT (:'r'::jsonb)->'invalidated_types' ? 'Post',
            'with data, Post invalidated: ' || :'r');
COMMIT;
RESET ROLE;
-- The posts were refreshed all the same, as their owner.
SELECT must((SELECT bool_and(data->>'author' = 'bob') FROM tv_post), 'posts not refreshed');

-- 2. Ids only: no post id either.
SET ROLE report_writer_270;
BEGIN;
UPDATE tb_user SET name = 'carol' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report(include_data => false) AS r \gset
SELECT must(reported(:'r', 'Post') = 0, 'ids only, posts listed: ' || :'r');
SELECT must(reported(:'r', 'User') = 1 AND reported(:'r', 'Comment') = 2,
            'ids only, readable rows missing: ' || :'r');
SELECT must((:'r'::jsonb)->'omitted_types' = '["Post"]', 'ids only, omitted_types: ' || :'r');
COMMIT;

-- 3. Asked for types: an unreadable type asked for is omitted too.
BEGIN;
UPDATE tb_user SET name = 'dave' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report(types => ARRAY['User', 'Post']) AS r \gset
SELECT must(reported(:'r', 'Post') = 0 AND reported(:'r', 'Comment') = 0
            AND reported(:'r', 'User') = 1, 'types: ' || :'r');
SELECT must((:'r'::jsonb)->'omitted_types' = '["Comment", "Post"]', 'types, omitted: ' || :'r');
COMMIT;
RESET ROLE;

-- 4. Collections: Comment.fk_post names posts, which the writer may not read.
SET pg_tviews.publish_changes = rows;
SET ROLE report_writer_270;
BEGIN;
UPDATE tb_comment SET fk_post = 2 WHERE pk_comment = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(reported(:'r', 'Comment') = 1, 'collections, comment missing: ' || :'r');
SELECT must(NOT (:'r'::jsonb)->'collections' ? 'Comment.fk_post',
            'collections, posts named: ' || :'r');
SELECT must((:'r'::jsonb)->'collections' ? 'Comment.fk_user',
            'collections, readable parent missing: ' || :'r');
COMMIT;
RESET ROLE;
RESET pg_tviews.publish_changes;

-- 5. A caller who may read every TVIEW sees every row and no omitted_types.
GRANT SELECT ON tv_post TO report_writer_270;
SET ROLE report_writer_270;
BEGIN;
UPDATE tb_user SET name = 'erin' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(reported(:'r', 'Post') = 2, 'readable, posts missing: ' || :'r');
SELECT must(NOT (:'r'::jsonb) ? 'omitted_types', 'readable, omitted_types: ' || :'r');
COMMIT;
RESET ROLE;

-- 6. A TVIEW in a schema the caller has no USAGE on is not readable: omitted,
--    without an error.
CREATE SCHEMA hidden_270;
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, body text);
INSERT INTO tb_note (pk_note, fk_user, body) VALUES (1, 1, 'n1');
SELECT tviews.pg_tviews_create('hidden_270.tv_note', $$
    SELECT n.pk_note, n.id, n.fk_user, jsonb_build_object('id', n.id, 'body', n.body) AS data
    FROM public.tb_note n $$);
GRANT SELECT, UPDATE ON tb_note TO report_writer_270;
GRANT SELECT ON hidden_270.tv_note TO report_writer_270;
SET ROLE report_writer_270;
BEGIN;
UPDATE tb_note SET body = 'n1!';
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(reported(:'r', 'Note') = 0, 'no schema USAGE, notes listed: ' || :'r');
SELECT must((:'r'::jsonb)->'omitted_types' ? 'Note', 'no schema USAGE, omitted: ' || :'r');
COMMIT;
RESET ROLE;

DROP EXTENSION pg_tviews CASCADE;
DROP SCHEMA hidden_270 CASCADE;
DROP OWNED BY report_writer_270;
DROP ROLE report_writer_270;
\echo 'regress_report_privileges: PASS'

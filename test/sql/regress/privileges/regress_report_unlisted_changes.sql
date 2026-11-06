-- Regression test: every change pg_tviews_flush_and_report() cannot list names its
-- type in `invalidated_types`.
--
-- The report lists only what the caller may read. A change it leaves out for that
-- reason still tells the caller that its cached results of the type may be stale:
--
--   - under row security, an inserted or updated row a read as the caller does not
--     return (as a deleted row already did);
--   - a collection whose parent TVIEW the caller may not read;
--   - a collection naming a parent a read as the caller does not return.
--
-- The type is named, never the row. Changes the report lists in full leave
-- `invalidated_types` empty. Both the explicit transaction and the one-statement
-- mutation function (autocommit) are checked.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_report_unlisted_changes.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'unlisted changes FAIL: %', what; END IF; END $$;

-- How many entries of `report` have type `t`.
CREATE FUNCTION reported(report jsonb, t text) RETURNS bigint LANGUAGE sql AS $$
    SELECT count(*) FROM jsonb_array_elements((report->'updated') || (report->'deleted')) e
    WHERE e->>'__typename' = t $$;

-- Whether `report` invalidates type `t`, and says it is truncated.
CREATE FUNCTION invalidates(report jsonb, t text) RETURNS boolean LANGUAGE sql AS $$
    SELECT (report->'invalidated_types') ? t AND (report->>'truncated')::boolean $$;

-- Whether `report` lists every change: nothing invalidated, not truncated.
CREATE FUNCTION complete(report jsonb) RETURNS boolean LANGUAGE sql AS $$
    SELECT report->'invalidated_types' = '[]' AND NOT (report->>'truncated')::boolean $$;

-- User ← Post ← Comment. Posts belong to a tenant; the caller's policy shows
-- tenant 'a' only.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                      name text NOT NULL);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, tenant text NOT NULL,
                      title text NOT NULL);
CREATE TABLE tb_comment (pk_comment int PRIMARY KEY,
                         id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
                         fk_post int NOT NULL REFERENCES tb_post, body text NOT NULL);
CREATE INDEX ON tb_post (fk_user);
CREATE INDEX ON tb_comment (fk_post);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO tb_post (pk_post, fk_user, tenant, title)
VALUES (1, 1, 'a', 'p1'), (2, 1, 'b', 'p2'), (3, 2, 'a', 'p3'), (4, 2, 'a', 'p4');
INSERT INTO tb_comment (pk_comment, fk_post, body) VALUES (1, 3, 'c1');
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('id', u.id, 'name', u.name) AS data
    FROM tb_user u $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, p.tenant,
           jsonb_build_object('id', p.id, 'title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
SELECT tviews.pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post, jsonb_build_object('id', c.id, 'body', c.body) AS data
    FROM tb_comment c $$);
ALTER TABLE tv_post ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_a ON tv_post USING (tenant = 'a');

DROP ROLE IF EXISTS report_unlisted_writer;
CREATE ROLE report_unlisted_writer;
GRANT USAGE ON SCHEMA tviews TO report_unlisted_writer;
GRANT SELECT, UPDATE ON tb_user, tb_post, tb_comment TO report_unlisted_writer;
GRANT SELECT ON tv_user, tv_post, tv_comment TO report_unlisted_writer;

-- A mutation function, called as one autocommit statement: it writes, then reports.
CREATE FUNCTION retitle(post int, title text, with_data boolean) RETURNS jsonb
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_post SET title = retitle.title WHERE pk_post = post;
    RETURN tviews.pg_tviews_flush_and_report(include_data => with_data);
END $$;
CREATE FUNCTION rehome(post int, tenant text, with_data boolean) RETURNS jsonb
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_post SET tenant = rehome.tenant WHERE pk_post = post;
    RETURN tviews.pg_tviews_flush_and_report(include_data => with_data);
END $$;

SET ROLE report_unlisted_writer;

-- 1. Rows the caller reads back are listed, and the report is complete.
SELECT retitle(1, 'p1!', true) AS r \gset
SELECT must(reported(:'r', 'Post') = 1 AND complete(:'r'), 'visible, function: ' || :'r');
BEGIN;
UPDATE tb_post SET title = 'p1!!' WHERE pk_post = 1;
SELECT tviews.pg_tviews_flush_and_report(include_data => false) AS r \gset
SELECT must(reported(:'r', 'Post') = 1 AND complete(:'r'), 'visible, ids only: ' || :'r');
COMMIT;

-- 2. A write that moves a row out of the caller's policy: the row is not listed,
--    its type is invalidated.
BEGIN;
UPDATE tb_post SET tenant = 'b' WHERE pk_post = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(reported(:'r', 'Post') = 0, 'moved out, listed: ' || :'r');
SELECT must(invalidates(:'r', 'Post'), 'moved out, with data: ' || :'r');
ROLLBACK;
BEGIN;
UPDATE tb_post SET tenant = 'b' WHERE pk_post = 1;
SELECT tviews.pg_tviews_flush_and_report(include_data => false) AS r \gset
SELECT must(reported(:'r', 'Post') = 0, 'moved out, ids only, listed: ' || :'r');
SELECT must(invalidates(:'r', 'Post'), 'moved out, ids only: ' || :'r');
ROLLBACK;
SELECT rehome(1, 'b', false) AS r \gset
SELECT must(reported(:'r', 'Post') = 0 AND invalidates(:'r', 'Post'),
            'moved out, function: ' || :'r');
RESET ROLE;
UPDATE tb_post SET tenant = 'a' WHERE pk_post = 1;
SET ROLE report_unlisted_writer;

-- 3. A write that refreshes rows the caller cannot read (alice's posts in both
--    tenants): the readable one is listed, the type is invalidated.
BEGIN;
UPDATE tb_user SET name = 'alicia' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(reported(:'r', 'Post') = 1 AND reported(:'r', 'User') = 1,
            'refreshed, readable rows: ' || :'r');
SELECT must(invalidates(:'r', 'Post') AND NOT (:'r'::jsonb)->'invalidated_types' ? 'User',
            'refreshed, with data: ' || :'r');
COMMIT;

-- 4. A collection whose parent TVIEW the caller may not read: the child type is
--    invalidated, and stands for all its collections.
RESET ROLE;
SET pg_tviews.publish_changes = rows;
REVOKE SELECT ON tv_post FROM report_unlisted_writer;
SET ROLE report_unlisted_writer;
BEGIN;
UPDATE tb_comment SET fk_post = 4 WHERE pk_comment = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(reported(:'r', 'Comment') = 1, 'parent unreadable, comment missing: ' || :'r');
SELECT must(NOT (:'r'::jsonb)->'collections' ? 'Comment.fk_post',
            'parent unreadable, collection listed: ' || :'r');
SELECT must(invalidates(:'r', 'Comment'), 'parent unreadable: ' || :'r');
ROLLBACK;
RESET ROLE;
GRANT SELECT ON tv_post TO report_unlisted_writer;
SET ROLE report_unlisted_writer;

-- 5. A collection between parents the caller reads: listed, nothing invalidated.
BEGIN;
UPDATE tb_comment SET fk_post = 4 WHERE pk_comment = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(jsonb_array_length((:'r'::jsonb)->'collections'->'Comment.fk_post') = 2
            AND complete(:'r'), 'readable parents: ' || :'r');
ROLLBACK;

-- 6. A collection naming a parent the caller's policy hides (a comment moved to
--    tenant b's post): that parent is not named, the child type is invalidated.
RESET ROLE;
SELECT id AS hidden_post FROM tb_post WHERE pk_post = 2 \gset
SET ROLE report_unlisted_writer;
BEGIN;
UPDATE tb_comment SET fk_post = 2 WHERE pk_comment = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(NOT jsonb_path_exists(:'r', '$.collections.*[*] ? (@ == $id)',
                                  jsonb_build_object('id', :'hidden_post')),
            'hidden parent named: ' || :'r');
SELECT must(invalidates(:'r', 'Comment'), 'hidden parent: ' || :'r');
ROLLBACK;
RESET ROLE;
RESET pg_tviews.publish_changes;

DROP EXTENSION pg_tviews CASCADE;
DROP OWNED BY report_unlisted_writer;
DROP ROLE report_unlisted_writer;
\echo 'regress_report_unlisted_changes: PASS'

-- Regression test (#230): pg_tviews_flush_and_report(types) reports only the
-- requested types.
--
-- A GraphQL cascade asks for the types its selection names. A username write
-- reaches the user, their posts (author.username) and the comments on them; a
-- mutation selecting User and Post only wants those. Everything is still flushed
-- and journaled; the rows of the other types are left out, not counted against
-- max_entities, and their types listed in `omitted_types`, apart from
-- `invalidated_types` (changed but cut by a limit).
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_flush_and_report_types.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#230 FAIL: %', what; END IF; END $$;

-- The SQLSTATE and message `stmt` fails with, NULL when it succeeds.
CREATE FUNCTION failure(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ' ' || SQLERRM; END $$;

-- How many entries of `report` have type `t`.
CREATE FUNCTION reported(report jsonb, t text) RETURNS bigint LANGUAGE sql AS $$
    SELECT count(*) FROM jsonb_array_elements((report->'updated') || (report->'deleted')) e
    WHERE e->>'__typename' = t $$;

-- User ← Post ← Comment, velocitybench's shape: one user, 10 posts, 5 comments each.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      username text NOT NULL);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author int NOT NULL REFERENCES tb_user, title text NOT NULL);
CREATE TABLE tb_comment (pk_comment int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_post int NOT NULL REFERENCES tb_post,
                         fk_author int NOT NULL REFERENCES tb_user, body text NOT NULL);
CREATE INDEX ON tb_post (fk_author);
CREATE INDEX ON tb_comment (fk_post);
CREATE INDEX ON tb_comment (fk_author);
INSERT INTO tb_user VALUES (1, DEFAULT, 'ann'), (2, DEFAULT, 'bob');
INSERT INTO tb_post (pk_post, fk_author, title) SELECT g, 1, 'p' || g FROM generate_series(1, 10) g;
INSERT INTO tb_comment (pk_comment, fk_post, fk_author, body)
    SELECT (p - 1) * 5 + c, p, 1, 'c' FROM generate_series(1, 10) p, generate_series(1, 5) c;
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('id', u.id, 'username', u.username) AS data
    FROM tb_user u $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('id', p.id, 'title', p.title,
                              'author', jsonb_build_object('id', u.id, 'username', u.username)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author $$);
SELECT tviews.pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_author, c.fk_post,
           jsonb_build_object('id', c.id, 'body', c.body,
                              'author', jsonb_build_object('id', u.id, 'username', u.username)) AS data
    FROM tb_comment c JOIN tb_user u ON u.pk_user = c.fk_author $$);

-- 1. A username write, User and Post asked for: 1 + 10 rows, Comment omitted.
BEGIN;
UPDATE tb_user SET username = 'ann2' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report(types => '{User,Post}') AS r \gset
SELECT must(reported(:'r', 'User') = 1 AND reported(:'r', 'Post') = 10
            AND reported(:'r', 'Comment') = 0, 'reported: ' || :'r');
SELECT must((:'r'::jsonb)->'omitted_types' = '["Comment"]'
            AND (:'r'::jsonb)->'invalidated_types' = '[]', 'omitted / invalidated: ' || :'r');
COMMIT;

-- 2. NULL reports everything, as before, without omitted_types.
BEGIN;
UPDATE tb_user SET username = 'ann3' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(reported(:'r', 'User') = 1 AND reported(:'r', 'Post') = 10
            AND reported(:'r', 'Comment') = 50 AND NOT (:'r'::jsonb ? 'omitted_types'),
            'NULL types: ' || :'r');
COMMIT;

-- 3. Left-out rows don't count against max_entities: 5 entries, User asked for,
--    50 comments changed: nothing invalidated, Comment and Post omitted.
BEGIN;
UPDATE tb_user SET username = 'ann4' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report(max_entities => 5, types => '{User}') AS r \gset
SELECT must(reported(:'r', 'User') = 1 AND (:'r'::jsonb)->'invalidated_types' = '[]'
            AND NOT (:'r'::jsonb->>'truncated')::boolean
            AND (:'r'::jsonb)->'omitted_types' = '["Comment", "Post"]', 'max_entities: ' || :'r');
COMMIT;

-- 4. An unknown type fails 22023 naming it, before anything is reported or reset.
BEGIN;
UPDATE tb_user SET username = 'ann5' WHERE pk_user = 1;
SELECT failure($$SELECT tviews.pg_tviews_flush_and_report(types => '{User,Psot}')$$) AS err \gset
SELECT must(:'err' LIKE '22023 %unknown%Psot%',
            'unknown type: ' || coalesce(:'err', 'it succeeded'));
SELECT must(reported(tviews.pg_tviews_flush_and_report(), 'User') = 1,
            'the failed call reset the journal');
COMMIT;
-- Type names follow the typename option.
SELECT tviews.pg_tviews_create_or_replace('tv_post', query, options || '{"typename": "Article"}')
FROM tviews.registry WHERE entity = 'post';
BEGIN;
UPDATE tb_user SET username = 'ann6' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report(types => '{Article}') AS r \gset
SELECT must(reported(:'r', 'Article') = 10 AND (:'r'::jsonb)->'omitted_types' = '["Comment", "User"]',
            'typename option: ' || :'r');
SELECT must(failure($$SELECT tviews.pg_tviews_flush_and_report(types => '{Post}')$$) LIKE '22023 %',
            'the entity''s default name still accepted');
COMMIT;

-- 5. reset clears the omitted rows too: a second call reports nothing.
BEGIN;
UPDATE tb_user SET username = 'ann7' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report(types => '{User}') AS r \gset
SELECT tviews.pg_tviews_flush_and_report() AS r2 \gset
SELECT must(jsonb_array_length((:'r2'::jsonb)->'updated') = 0, 'omitted rows survived reset: ' || :'r2');
-- An empty list is valid: everything omitted.
UPDATE tb_user SET username = 'ann8' WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report(types => '{}') AS r \gset
SELECT must(jsonb_array_length((:'r'::jsonb)->'updated') = 0
            AND (:'r'::jsonb)->'omitted_types' = '["Article", "Comment", "User"]', 'empty types: ' || :'r');
COMMIT;

-- 6. include_data => false reads no omitted TVIEW.
BEGIN;
UPDATE tb_user SET username = 'ann9' WHERE pk_user = 1;
SELECT seq_scan + coalesce(idx_scan, 0) AS before FROM pg_stat_xact_user_tables
WHERE relname = 'tv_comment' \gset
SELECT tviews.pg_tviews_flush_and_report(include_data => false, types => '{User}') AS r \gset
SELECT must(seq_scan + coalesce(idx_scan, 0) - :before = 0, 'tv_comment was read by the report')
FROM pg_stat_xact_user_tables WHERE relname = 'tv_comment';
COMMIT;

\echo 'flush_and_report types (#230): PASS'

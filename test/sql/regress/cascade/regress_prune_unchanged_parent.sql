-- Regression test for issue #85: stop propagation at a parent whose row did not change.
--
-- With #72 an unchanged row is not rewritten, but propagation still fanned out from
-- every queued key: a no-op on a user recomputed all of that user's posts and all of
-- their comments, only to write nothing. Propagation along an edge where the parent
-- embeds the child's computed document now stops at a child row that did not change.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_prune_unchanged_parent.sql
-- expect-output: prune_unchanged_parent: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name    TEXT
);
CREATE TABLE tb_post (
    pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    title   TEXT
);
CREATE TABLE tb_comment (
    pk_comment BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id         UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_post    BIGINT NOT NULL REFERENCES tb_post(pk_post),
    body       TEXT
);
INSERT INTO tb_user (name) VALUES ('ann'), ('bob');
INSERT INTO tb_post (fk_user, title) SELECT 1, 'p' || g FROM generate_series(1, 20) g;
INSERT INTO tb_comment (fk_post, body) SELECT p, 'c' FROM generate_series(1, 20) p, generate_series(1, 3);

CREATE TABLE tv_user AS
SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user;
CREATE TABLE tv_post AS
SELECT p.pk_post, p.id, p.fk_user,
       jsonb_build_object('title', p.title, 'author', tv_user.data) AS data
FROM tb_post p JOIN tv_user ON tv_user.pk_user = p.fk_user;
CREATE TABLE tv_comment AS
SELECT c.pk_comment, c.id, c.fk_post,
       jsonb_build_object('body', c.body, 'post', tv_post.data) AS data
FROM tb_comment c JOIN tv_post ON tv_post.pk_post = c.fk_post;

CREATE FUNCTION stat(k TEXT) RETURNS BIGINT LANGUAGE sql AS
$$ SELECT (pg_tviews_queue_stats()->>k)::bigint $$;
CREATE FUNCTION must(ok BOOLEAN, msg TEXT) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION '#85 FAIL: %', msg; END IF; END $$;
CREATE FUNCTION in_sync() RETURNS BOOLEAN LANGUAGE sql AS $$
    SELECT (SELECT count(*) FROM tviews.public__tv_post v FULL JOIN tv_post t USING (pk_post)
            WHERE t.data IS DISTINCT FROM v.data) = 0
       AND (SELECT count(*) FROM tviews.public__tv_comment v FULL JOIN tv_comment t USING (pk_comment)
            WHERE t.data IS DISTINCT FROM v.data) = 0
$$;

SET pg_tviews.direct_patch_enabled = off;

-- ========================================================================
-- Cycle 1: a no-op on the user stops at the user
-- ========================================================================
SELECT stat('view_recomputes') AS r0, stat('propagation_pruned') AS p0 \gset
UPDATE tb_user SET name = name WHERE pk_user = 1;
SELECT must(stat('view_recomputes') - :r0 = 1,
            format('no-op recomputed %s rows, expected only the user', stat('view_recomputes') - :r0));
SELECT must(stat('propagation_pruned') - :p0 >= 1, 'no propagation edge was pruned');

-- ========================================================================
-- Cycle 2: a real change still reaches every post and comment
-- ========================================================================
SELECT stat('view_recomputes') AS r1 \gset
UPDATE tb_user SET name = 'ann2' WHERE pk_user = 1;
SELECT must(stat('view_recomputes') - :r1 = 81,
            format('rename recomputed %s rows, expected 1 user + 20 posts + 60 comments',
                   stat('view_recomputes') - :r1));
SELECT must(in_sync(), 'posts/comments out of sync after the rename');
SELECT must((SELECT data->'post'->'author'->>'name' FROM tv_comment WHERE pk_comment = 1) = 'ann2',
            'rename did not reach the comments');

-- ========================================================================
-- Cycle 3: a change that stops midway (post unchanged) spares the comments
-- ========================================================================
-- tb_post.fk_user moves post 1 to bob: post 1 changes, its comments must follow.
UPDATE tb_post SET fk_user = 2 WHERE pk_post = 1;
SELECT must(in_sync(), 'out of sync after moving a post');
-- A no-op on a post leaves its comments alone.
SELECT stat('view_recomputes') AS r2 \gset
UPDATE tb_post SET title = title WHERE pk_post = 2;
SELECT must(stat('view_recomputes') - :r2 = 1,
            format('post no-op recomputed %s rows, expected 1', stat('view_recomputes') - :r2));

-- ========================================================================
-- Cycle 4: the direct-patch path prunes too, and stays correct
-- ========================================================================
SET pg_tviews.direct_patch_enabled = on;
UPDATE tb_user SET name = name WHERE pk_user = 1;
UPDATE tb_user SET name = 'ann3' WHERE pk_user = 1;
SELECT must(in_sync(), 'out of sync on the direct-patch path');

\echo 'prune_unchanged_parent: PASS'

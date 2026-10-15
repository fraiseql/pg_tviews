-- Regression test (#228): a column copied two joins away is patched in place.
--
-- tv_comment embeds its post's author: tb_comment → tb_post (fk_post) → tb_user
-- (fk_author). A username write reaches the comments on the user's posts. They
-- were recomputed from the backing view, one by one; the fan-out patch now finds
-- them through the table in between (the user's posts, by tb_post.fk_author) and
-- writes the new name at post.author.username in one statement.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_fanout_two_hop.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#228 FAIL: %', what; END IF; END $$;

-- The sequential scans of `rel` while `write` runs, in the caller's transaction.
CREATE FUNCTION seq_scans_during(rel regclass, write text) RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE before bigint;
BEGIN
    SELECT seq_scan INTO before FROM pg_stat_xact_user_tables WHERE relid = rel;
    EXECUTE write;
    RETURN (SELECT seq_scan FROM pg_stat_xact_user_tables WHERE relid = rel) - before;
END $$;

-- tv_comment's patched and recomputed rows, read after each write's commit.
CREATE FUNCTION counts() RETURNS TABLE (patched bigint, recomputed bigint) LANGUAGE sql AS $$
    SELECT patch_applied, view_recomputes FROM tviews.stats WHERE entity = 'comment' $$;

-- 200 users, 2,000 posts (10 each), 10,000 comments (5 per post).
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      username text NOT NULL);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author int NOT NULL REFERENCES tb_user, title text NOT NULL);
CREATE TABLE tb_comment (pk_comment int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_post int NOT NULL REFERENCES tb_post, body text NOT NULL);
CREATE INDEX ON tb_post (fk_author);
CREATE INDEX ON tb_comment (fk_post);
INSERT INTO tb_user (pk_user, username) SELECT g, 'user' || g FROM generate_series(1, 200) g;
INSERT INTO tb_post (pk_post, fk_author, title) SELECT g, 1 + g % 200, 'p' || g FROM generate_series(1, 2000) g;
INSERT INTO tb_comment (pk_comment, fk_post, body) SELECT g, 1 + g % 2000, 'c' FROM generate_series(1, 10000) g;
SELECT tviews.pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post,
           jsonb_build_object('body', c.body,
               'post', jsonb_build_object('title', p.title,
                   'author', jsonb_build_object('id', u.id, 'username', u.username))) AS data
    FROM tb_comment c
    JOIN tb_post p ON p.pk_post = c.fk_post
    JOIN tb_user u ON u.pk_user = p.fk_author $$);
ANALYZE tb_user, tb_post, tb_comment, tv_comment;

-- 1. A username write: the 50 comments on the user's 10 posts are patched, none
--    recomputed, and no table is scanned whole to find them.
SELECT tviews.pg_tviews_stats_reset();
SELECT must(seq_scans_during('tv_comment', $$UPDATE tb_user SET username = 'one' WHERE pk_user = 7$$) = 0,
            'the write scanned tv_comment');
SELECT must((patched, recomputed) = (50, 0), format('one user: %s patched, %s recomputed', patched, recomputed))
FROM counts();
SELECT must((SELECT count(*) FROM tv_comment WHERE data #>> '{post,author,username}' = 'one') = 50,
            'not every comment on the user''s posts holds the new name');
SELECT assert_fresh('tv_comment', 'pk_comment', 'a username write');
SELECT must(seq_scans_during('tb_post', $$UPDATE tb_user SET username = 'one again' WHERE pk_user = 7$$) = 0,
            'the write scanned tb_post');

-- 2. Several users in one statement.
SELECT tviews.pg_tviews_stats_reset();
UPDATE tb_user SET username = username || '!' WHERE pk_user IN (8, 9, 10);
SELECT must((patched, recomputed) = (150, 0), format('three users: %s patched, %s recomputed', patched, recomputed))
FROM counts();
SELECT assert_fresh('tv_comment', 'pk_comment', 'three users renamed');

-- 3. A user without posts: nothing written.
INSERT INTO tb_user (pk_user, username) VALUES (999, 'nobody');
SELECT tviews.pg_tviews_stats_reset();
UPDATE tb_user SET username = 'still nobody' WHERE pk_user = 999;
SELECT must((patched, recomputed) = (0, 0), format('no posts: %s patched, %s recomputed', patched, recomputed))
FROM counts();

-- 4. A post moved to another author in the same statement as a rename: the
--    moved post's comments are recomputed, the others patched, all fresh.
WITH moved AS (UPDATE tb_post SET fk_author = 12 WHERE pk_post = 3 RETURNING 1)
UPDATE tb_user SET username = 'twelve' WHERE pk_user = 12;
SELECT assert_fresh('tv_comment', 'pk_comment', 'a rename and a move in one statement');
SELECT must((SELECT count(*) FROM tv_comment WHERE data #>> '{post,author,username}' = 'twelve') = 55,
            'the moved post''s comments do not show their new author');

-- 5. REPEATABLE READ: the lookup through tb_post is checked like the patch's own.
BEGIN ISOLATION LEVEL REPEATABLE READ;
UPDATE tb_user SET username = 'rr' WHERE pk_user = 13;
COMMIT;
SELECT assert_fresh('tv_comment', 'pk_comment', 'a rename at REPEATABLE READ');

-- 6. Without an index on tb_post.fk_author the comments are still found (as the
--    recompute found them, scanning tb_post) and patched.
DROP INDEX tb_post_fk_author_idx;
SELECT tviews.pg_tviews_stats_reset();
UPDATE tb_user SET username = 'no index' WHERE pk_user = 14;
SELECT must((patched, recomputed) = (50, 0), format('no index: %s patched, %s recomputed', patched, recomputed))
FROM counts();
SELECT assert_fresh('tv_comment', 'pk_comment', 'a rename without an index on tb_post.fk_author');

-- 7. With the patch off, the same write recomputes, as before.
SET pg_tviews.direct_patch_enabled = off;
SELECT tviews.pg_tviews_stats_reset();
UPDATE tb_user SET username = 'recomputed' WHERE pk_user = 15;
RESET pg_tviews.direct_patch_enabled;
SELECT must((patched, recomputed) = (0, 50), format('patch off: %s patched, %s recomputed', patched, recomputed))
FROM counts();
SELECT assert_fresh('tv_comment', 'pk_comment', 'a rename recomputed');

-- 8. Keys of the table in between that are no integers (a uuid post): the write
--    still succeeds, its rows recomputed.
CREATE TABLE tb_author (pk_author int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                        name text NOT NULL);
CREATE TABLE tb_article (pk_article uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                         fk_author int NOT NULL REFERENCES tb_author, title text NOT NULL);
CREATE TABLE tb_reply (pk_reply int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_article uuid NOT NULL REFERENCES tb_article, body text NOT NULL);
INSERT INTO tb_author (pk_author, name) VALUES (1, 'ann'), (2, 'bob');
INSERT INTO tb_article (fk_author, title) SELECT 1 + g % 2, 'a' || g FROM generate_series(1, 4) g;
INSERT INTO tb_reply (pk_reply, fk_article, body)
    SELECT g, (SELECT pk_article FROM tb_article ORDER BY title OFFSET g % 4 LIMIT 1), 'r'
    FROM generate_series(1, 8) g;
SELECT tviews.pg_tviews_create('tv_reply', $$
    SELECT r.pk_reply, r.id, r.fk_article,
           jsonb_build_object('body', r.body,
               'article', jsonb_build_object('title', a.title,
                   'author', jsonb_build_object('name', u.name))) AS data
    FROM tb_reply r
    JOIN tb_article a ON a.pk_article = r.fk_article
    JOIN tb_author u ON u.pk_author = a.fk_author $$);
UPDATE tb_author SET name = 'ann2' WHERE pk_author = 1;
SELECT assert_fresh('tv_reply', 'pk_reply', 'a rename through uuid keys');

-- 9. The plan: tb_user fans out by fk_post, through tb_post.
SELECT must((SELECT t->'fanout'->>'lookup_col' = 'fk_post' AND t->'fanout' ? 'via'
             FROM tviews.pg_tview_meta m, jsonb_array_elements(m.plan->'tables') t
             WHERE m.entity = 'comment' AND (t->>'relid')::oid = 'tb_user'::regclass::oid),
            'tb_user has no fan-out through tb_post');

\echo 'fan-out two hops (#228): PASS'

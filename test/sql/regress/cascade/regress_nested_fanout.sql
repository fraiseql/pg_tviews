-- Regression test (#224): the fan-out patch reaches fields of nested objects.
--
-- FraiseQL nests a joined entity: 'author', jsonb_build_object('bio', u.bio, …).
-- A write to tb_user.bio used to recompute every post (and comment) embedding the
-- author through its backing view, because the fan-out patch only wrote top-level
-- keys. It now writes the nested fields in place: the embedding rows count as
-- patched, not recomputed, and stay equal to their views.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_nested_fanout.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      username text, bio text, city text, score numeric);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author int NOT NULL REFERENCES tb_user, title text);
CREATE TABLE tb_comment (pk_comment int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_post int NOT NULL REFERENCES tb_post, fk_author int NOT NULL REFERENCES tb_user,
                         content text);
INSERT INTO tb_user SELECT g, gen_random_uuid(), 'user' || g, 'bio ' || g, 'c' || g, g FROM generate_series(1, 4) g;
INSERT INTO tb_post SELECT g, gen_random_uuid(), 1 + g % 4, 'title ' || g FROM generate_series(1, 20) g;
INSERT INTO tb_comment SELECT g, gen_random_uuid(), 1 + g % 20, 1 + (g * 3) % 4, 'c' || g FROM generate_series(1, 60) g;

SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('id', id, 'username', username, 'bio', bio) AS data FROM tb_user $$);
-- author.bio and author.username nested one level, author.address.city two.
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('id', p.id, 'title', p.title,
               'author', jsonb_build_object('id', u.id, 'username', u.username, 'bio', u.bio,
                   'address', jsonb_build_object('city', u.city))) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author $$);
SELECT tviews.pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post, c.fk_author,
           jsonb_build_object('content', c.content,
               'author', jsonb_build_object('username', u.username, 'score', u.score),
               'post', jsonb_build_object('title', p.title)) AS data
    FROM tb_comment c JOIN tb_user u ON u.pk_user = c.fk_author JOIN tb_post p ON p.pk_post = c.fk_post $$);

CREATE FUNCTION assert_fresh(step text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE entity text; d bigint;
BEGIN
    FOREACH entity IN ARRAY ARRAY['user', 'post', 'comment'] LOOP
        EXECUTE format(
            'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tviews.public__tv_%1$s)
                         UNION ALL (SELECT pk_%1$s, data FROM tviews.public__tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d', entity) INTO d;
        IF d <> 0 THEN
            RAISE EXCEPTION '#224 FAIL after %: tv_% diverges from its view (% rows)', step, entity, d;
        END IF;
    END LOOP;
END $$;
-- tviews.stats are merged when a transaction ends: snapshot, write, then read
-- each TVIEW's patched and recomputed rows since, `entity:patched/recomputed`.
CREATE TABLE snap (s jsonb);
CREATE FUNCTION snap() RETURNS void LANGUAGE sql AS $$
    DELETE FROM snap;
    INSERT INTO snap SELECT jsonb_object_agg(entity, jsonb_build_array(patch_applied, view_recomputes))
    FROM tviews.stats $$;
CREATE FUNCTION counted() RETURNS text LANGUAGE sql AS $$
    WITH after AS (SELECT jsonb_object_agg(entity, jsonb_build_array(patch_applied, view_recomputes)) a
                   FROM tviews.stats)
    SELECT string_agg(format('%s:%s/%s', k,
               (a->k->>0)::bigint - coalesce((s->k->>0)::bigint, 0),
               (a->k->>1)::bigint - coalesce((s->k->>1)::bigint, 0)), ' ' ORDER BY k)
    FROM after, snap, jsonb_object_keys(a) k $$;
CREATE FUNCTION must(got text, want text, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF got IS DISTINCT FROM want THEN RAISE EXCEPTION '#224 FAIL: %: got %, want %', what, got, want; END IF; END $$;

-- 1. bio: tv_user's own row patched, user 2's 5 posts patched, no view recompute.
SELECT snap();
UPDATE tb_user SET bio = 'new bio' WHERE pk_user = 2;
SELECT must(counted(), 'comment:0/0 post:5/0 user:1/0', 'bio');
SELECT assert_fresh('bio');
-- 2. username: the posts and the comments embedding the user, both patched.
SELECT snap();
UPDATE tb_user SET username = 'renamed' WHERE pk_user = 3;
SELECT must(counted(), 'comment:15/0 post:5/0 user:1/0', 'username');
SELECT assert_fresh('username');
-- 3. Two levels down, and two fields of one object in one write.
SELECT snap();
UPDATE tb_user SET city = 'Lyon', bio = 'b2', username = 'u2' WHERE pk_user = 4;
SELECT must(counted(), 'comment:15/0 post:5/0 user:1/0', 'city + bio + username');
SELECT assert_fresh('city + bio + username');
-- 4. NULLs are written as JSON null, like the view.
UPDATE tb_user SET bio = NULL, city = NULL WHERE pk_user = 1;
SELECT assert_fresh('nulls');
-- 5. A statement over several users, and the same row twice in a transaction.
BEGIN;
UPDATE tb_user SET bio = bio || '!';
UPDATE tb_user SET bio = 'again' WHERE pk_user = 2;
COMMIT;
SELECT assert_fresh('bulk and twice');
-- 6. A column read outside data (the key) still maps the rows: recomputed.
SELECT snap();
UPDATE tb_post SET title = 'moved' WHERE pk_post = 1;
SELECT must(counted(), 'comment:3/0 post:1/0 user:0/0', 'title fans out into comment.post.title');
SELECT assert_fresh('title');

-- 7. A column copied at two paths is written at both; a computed field (upper())
--    is not copied, so a write to its column recomputes the rows.
SELECT tviews.pg_tviews_create('tv_byline', $$
    SELECT p.pk_post AS pk_byline, p.id, p.fk_author,
           jsonb_build_object('username', u.username,
               'author', jsonb_build_object('username', u.username, 'shout', upper(u.bio))) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_author $$);
SELECT snap();
UPDATE tb_user SET username = 'twice' WHERE pk_user = 1;
SELECT must(split_part(substring(counted() FROM 'byline:[0-9/]+'), ':', 2), '5/0', 'two paths');
SELECT snap();
UPDATE tb_user SET bio = 'loud' WHERE pk_user = 1;
SELECT must(split_part(substring(counted() FROM 'byline:[0-9/]+'), ':', 2), '0/5', 'computed field');
SELECT must((SELECT count(*) FROM tv_byline WHERE data->>'username' = 'twice'
             AND data->'author'->>'username' = 'twice' AND data->'author'->>'shout' = 'LOUD')::text,
            '5', 'byline rows');
CREATE OR REPLACE FUNCTION assert_byline() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT pk_byline, data FROM tv_byline EXCEPT SELECT pk_byline, data FROM tviews.public__tv_byline) THEN
        RAISE EXCEPTION '#224 FAIL: tv_byline diverges';
    END IF;
END $$;
SELECT assert_byline();

SELECT 'issue #224 nested fan-out: PASS' AS result;
-- expect-output: issue #224 nested fan-out: PASS

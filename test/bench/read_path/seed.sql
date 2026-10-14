-- Read composition: the #228 array shape stored two ways, on the shapes.sql seed
-- (2k users, -v posts=20000 posts, 100k comments: 5 per post at 20000, 50 per user as author), with
-- deterministic ids so that both databases hold the same documents.
--   -v design=embedded: tv_post.data.comments[] (test/bench/write_path/shapes.sql, shape=array)
--   -v design=composed: tv_post without comments + tv_comment carrying fk_post;
--                       the reader composes post.comments from tv_comment.
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
CREATE SCHEMA bench;
SET search_path = bench, public;

CREATE TABLE tb_user (
    pk_user int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    username text NOT NULL UNIQUE
);
CREATE TABLE tb_post (
    pk_post int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    title text NOT NULL,
    fk_author int NOT NULL REFERENCES tb_user
);
CREATE INDEX ON tb_post (fk_author);
CREATE TABLE tb_comment (
    pk_comment int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_post int NOT NULL REFERENCES tb_post,
    fk_author int NOT NULL REFERENCES tb_user,
    content text NOT NULL
);
CREATE INDEX ON tb_comment (fk_post);
CREATE INDEX ON tb_comment (fk_author);

INSERT INTO tb_user (id, username)
SELECT md5('u' || g)::uuid, 'user' || g FROM generate_series(1, 2000) g;
INSERT INTO tb_post (id, title, fk_author)
SELECT md5('p' || g)::uuid, 'Title ' || g, 1 + (g % 2000) FROM generate_series(1, :posts) g;
INSERT INTO tb_comment (id, fk_post, fk_author, content)
SELECT md5('c' || g)::uuid, 1 + (g % :posts), 1 + ((g * 7) % 2000), repeat('comment ', 15)
FROM generate_series(1, 100000) g;

-- embedded: shapes.sql's tv_post, verbatim
SELECT tviews.pg_tviews_create('bench.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('id', p.id::text, 'title', p.title,
               'comments', COALESCE((
                   SELECT jsonb_agg(jsonb_build_object('id', c.id::text, 'content', c.content,
                              'author', jsonb_build_object('id', u.id::text, 'username', u.username))
                          ORDER BY c.pk_comment)
                   FROM bench.tb_comment c JOIN bench.tb_user u ON u.pk_user = c.fk_author
                   WHERE c.fk_post = p.pk_post), '[]'::jsonb)) AS data
    FROM bench.tb_post p $$)
WHERE :'design' = 'embedded';

-- composed: the same documents split at the array
SELECT tviews.pg_tviews_create('bench.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author,
           jsonb_build_object('id', p.id::text, 'title', p.title) AS data
    FROM bench.tb_post p $$)
WHERE :'design' = 'composed';
SELECT tviews.pg_tviews_create('bench.tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post, c.fk_author,
           jsonb_build_object('id', c.id::text, 'content', c.content,
               'author', jsonb_build_object('id', u.id::text, 'username', u.username)) AS data
    FROM bench.tb_comment c JOIN bench.tb_user u ON u.pk_user = c.fk_author $$)
WHERE :'design' = 'composed';
VACUUM ANALYZE;

-- #228 shapes: a column copied into an array of embedded rows, and a column copied
-- across two hops. Seed as schema.sql: 2k users, 20k posts (10 per user), 100k
-- comments (5 per post, 50 per user as author). One TVIEW per database, chosen
-- with -v shape=array or -v shape=two_hop, so that a username write measures one
-- shape.
--   array:   tv_post.data.comments[].author.username (the comments' authors)
--   two_hop: tv_comment.data.post.author.username  (the post's author)
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

INSERT INTO tb_user (username) SELECT 'user' || g FROM generate_series(1, 2000) g;
INSERT INTO tb_post (title, fk_author)
SELECT 'Title ' || g, 1 + (g % 2000) FROM generate_series(1, 20000) g;
INSERT INTO tb_comment (fk_post, fk_author, content)
SELECT 1 + (g % 20000), 1 + ((g * 7) % 2000), repeat('comment ', 15)
FROM generate_series(1, 100000) g;

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
WHERE :'shape' = 'array';
SELECT tviews.pg_tviews_create('bench.tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post,
           jsonb_build_object('id', c.id::text, 'content', c.content,
               'post', jsonb_build_object('id', p.id::text, 'title', p.title,
                   'author', jsonb_build_object('id', u.id::text, 'username', u.username))) AS data
    FROM bench.tb_comment c
    JOIN bench.tb_post p ON p.pk_post = c.fk_post
    JOIN bench.tb_user u ON u.pk_user = p.fk_author $$)
WHERE :'shape' = 'two_hop';
VACUUM ANALYZE;

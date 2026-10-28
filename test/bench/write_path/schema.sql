-- velocitybench-shaped write benchmark (#224): FraiseQL CQRS tables, TVIEWs that
-- nest the joined entity (`author`, `post`) as FraiseQL's codegen does.
-- Seed: 2k users, 20k posts (10 per user), 100k comments (50 per user).
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
CREATE SCHEMA bench;
SET search_path = bench, public;

CREATE TABLE tb_user (
    pk_user int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier text NOT NULL UNIQUE,
    email text NOT NULL UNIQUE,
    username text NOT NULL UNIQUE,
    full_name text NOT NULL,
    bio text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE tb_post (
    pk_post int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier text NOT NULL UNIQUE,
    title text NOT NULL,
    content text NOT NULL,
    fk_author int NOT NULL REFERENCES tb_user ON DELETE CASCADE,
    published boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ON tb_post (fk_author);
CREATE TABLE tb_comment (
    pk_comment int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier text UNIQUE,
    fk_post int NOT NULL REFERENCES tb_post ON DELETE CASCADE,
    fk_author int NOT NULL REFERENCES tb_user ON DELETE CASCADE,
    content text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ON tb_comment (fk_post);
CREATE INDEX ON tb_comment (fk_author);
-- Read by no TVIEW: what an installed pg_tviews costs a table it does not maintain.
CREATE TABLE tb_audit (
    pk_audit bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    at timestamptz NOT NULL DEFAULT now(),
    note text NOT NULL
);

SELECT tviews.pg_tviews_create('bench.tv_user', $$
    SELECT u.pk_user, u.id, u.identifier,
           jsonb_build_object('id', u.id::text, 'identifier', u.identifier, 'email', u.email,
               'username', u.username, 'full_name', u.full_name, 'bio', u.bio,
               'created_at', u.created_at, 'updated_at', u.updated_at) AS data
    FROM bench.tb_user u $$);
SELECT tviews.pg_tviews_create('bench.tv_post', $$
    SELECT p.pk_post, p.id, p.identifier, p.fk_author,
           jsonb_build_object('id', p.id::text, 'identifier', p.identifier, 'title', p.title,
               'content', p.content, 'published', p.published,
               'created_at', p.created_at, 'updated_at', p.updated_at,
               'author', jsonb_build_object('id', u.id::text, 'username', u.username,
                   'full_name', u.full_name, 'bio', u.bio)) AS data
    FROM bench.tb_post p JOIN bench.tb_user u ON u.pk_user = p.fk_author $$);
SELECT tviews.pg_tviews_create('bench.tv_comment', $$
    SELECT c.pk_comment, c.id, c.identifier, c.fk_author, c.fk_post,
           jsonb_build_object('id', c.id::text, 'identifier', c.identifier, 'content', c.content,
               'created_at', c.created_at, 'updated_at', c.updated_at,
               'author', jsonb_build_object('id', u.id::text, 'username', u.username),
               'post', jsonb_build_object('id', p.id::text, 'title', p.title)) AS data
    FROM bench.tb_comment c
    JOIN bench.tb_user u ON u.pk_user = c.fk_author
    JOIN bench.tb_post p ON p.pk_post = c.fk_post $$);

INSERT INTO tb_user (identifier, email, username, full_name, bio)
SELECT 'u' || g, 'u' || g || '@x.io', 'user' || g, 'User ' || g, repeat('bio text ', 20)
FROM generate_series(1, 2000) g;
INSERT INTO tb_post (identifier, title, content, fk_author, published)
SELECT 'p' || g, 'Title ' || g, repeat('content words ', 60), 1 + (g % 2000), true
FROM generate_series(1, 20000) g;
INSERT INTO tb_comment (identifier, content, fk_post, fk_author)
SELECT 'c' || g, repeat('comment ', 15), 1 + (g % 20000), 1 + ((g * 7) % 2000)
FROM generate_series(1, 100000) g;
VACUUM ANALYZE;

-- A child TVIEW embedding its parent through a plain view (not through the
-- parent TVIEW) follows every write to either table.
--
-- tv_post reads tb_user through user_prepared, so tb_user is a mapped table in
-- post's plan (keyed back through fk_user), not an embed. A parent update
-- reaches all of its children and none of another parent's; a child update does
-- not rewrite the parent row; moving a child to another parent, inserting and
-- deleting a child all leave both TVIEWs fresh.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_fk_lineage.sql
--
-- expect-output: fk lineage: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'fk lineage FAIL: %', what; END IF; END $$;
CREATE FUNCTION both_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM assert_fresh('tv_user', 'pk_user', label);
    PERFORM assert_fresh('tv_post', 'pk_post', label);
END $$;

CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT NOT NULL,
    email TEXT
);
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user INTEGER NOT NULL REFERENCES tb_user(pk_user),
    title TEXT NOT NULL,
    content TEXT
);
INSERT INTO tb_user (name, email) VALUES ('Alice', 'alice@example.com'), ('Bob', 'bob@example.com');
INSERT INTO tb_post (fk_user, title, content) VALUES
    (1, 'Alice Post 1', 'Content 1'),
    (1, 'Alice Post 2', 'Content 2'),
    (1, 'Alice Post 3', 'Content 3'),
    (2, 'Bob Post 1', 'Bob content');

CREATE VIEW user_prepared AS
SELECT pk_user, id, jsonb_build_object('id', id::text, 'name', name, 'email', email) AS data
FROM tb_user;
CREATE VIEW post_prepared AS
SELECT p.pk_post, p.id, p.fk_user, u.id AS user_id,
       jsonb_build_object('id', p.id::text, 'title', p.title, 'content', p.content,
                          'author', u.data) AS data
FROM tb_post p JOIN user_prepared u ON u.pk_user = p.fk_user;

SELECT pg_tviews_create('tv_user', 'SELECT pk_user, id, data FROM user_prepared');
SELECT pg_tviews_create('tv_post', 'SELECT pk_post, id, fk_user, user_id, data FROM post_prepared');

-- 1. Initial population, with nested authors.
SELECT both_fresh('pg_tviews_create');
SELECT must((SELECT count(*) FROM tv_user) = 2 AND (SELECT count(*) FROM tv_post) = 4
            AND (SELECT count(*) FROM tv_post WHERE data->'author'->>'name' = 'Alice') = 3
            AND (SELECT count(*) FROM tv_post WHERE data->'author'->>'name' = 'Bob') = 1,
            'initial users/posts or nested authors wrong');

-- 2. A parent update reaches all of its children, and only them.
UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1;
SELECT both_fresh('an UPDATE of tb_user.name');
SELECT must((SELECT data->>'name' FROM tv_user WHERE pk_user = 1) = 'Alice Updated',
            'tv_user row 1 not refreshed');
SELECT must((SELECT count(*) FROM tv_post WHERE fk_user = 1 AND data->'author'->>'name' = 'Alice Updated') = 3,
            'user rename did not reach all 3 of her posts');
SELECT must((SELECT data->'author'->>'name' FROM tv_post WHERE fk_user = 2) = 'Bob',
            'Bob''s post changed by Alice''s rename');

-- 3. Two parent fields at once.
UPDATE tb_user SET name = 'Alice V2', email = 'alice.v2@example.com' WHERE pk_user = 1;
SELECT both_fresh('an UPDATE of two tb_user fields');
SELECT must((SELECT (data->'author'->>'name', data->'author'->>'email') FROM tv_post WHERE pk_post = 1)
            = ('Alice V2'::text, 'alice.v2@example.com'::text),
            'post 1 author not (Alice V2, alice.v2@example.com)');

-- 4. A child update does not rewrite the parent row (a rewrite changes its ctid).
CREATE TEMP TABLE user_before AS SELECT ctid AS tid FROM tv_user WHERE pk_user = 1;
UPDATE tb_post SET title = 'Alice Post 1 Updated' WHERE pk_post = 1;
SELECT both_fresh('an UPDATE of tb_post.title');
SELECT must((SELECT data->>'title' FROM tv_post WHERE pk_post = 1) = 'Alice Post 1 Updated',
            'post 1 title not refreshed');
SELECT must((SELECT ctid FROM tv_user WHERE pk_user = 1) = (SELECT tid FROM user_before),
            'a post update rewrote its parent user row');

-- 5. Moving a child to another parent.
UPDATE tb_post SET fk_user = 2 WHERE pk_post = 1;
SELECT both_fresh('re-pointing tb_post.fk_user');
SELECT must((SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 1) = 'Bob',
            'moved post 1 does not show Bob as author');
SELECT must((SELECT count(*) FROM tv_post WHERE fk_user = 1) = 2
            AND (SELECT count(*) FROM tv_post WHERE fk_user = 2) = 2,
            'post counts per user not 2/2 after the move');

-- 6. A new child embeds the current parent.
INSERT INTO tb_post (fk_user, title, content) VALUES (1, 'New Alice Post', 'New content');
SELECT both_fresh('an INSERT into tb_post');
SELECT must((SELECT (data->'author'->>'name', data->'author'->>'email') FROM tv_post
             WHERE data->>'title' = 'New Alice Post')
            = ('Alice V2'::text, 'alice.v2@example.com'::text),
            'new post does not embed the current Alice');

-- 7. Deleting a child leaves its parent.
DELETE FROM tb_post WHERE pk_post = 2;
SELECT both_fresh('a DELETE from tb_post');
SELECT must(NOT EXISTS (SELECT 1 FROM tv_post WHERE pk_post = 2), 'deleted post 2 still in tv_post');
SELECT must(EXISTS (SELECT 1 FROM tv_user WHERE pk_user = 1), 'deleting a post removed its user');

-- 8. Plans: one local path and no embeds each; post maps tb_user.
SELECT must(NOT EXISTS (SELECT 1 FROM pg_tview_meta
                        WHERE jsonb_array_length(plan->'paths') <> 1
                           OR jsonb_array_length(plan->'embeds') <> 0),
            'expected 1 local path and 0 embeds per TVIEW');
SELECT must(EXISTS (SELECT 1 FROM pg_tview_meta m, jsonb_array_elements(m.plan->'tables') t
                    WHERE m.entity = 'post' AND t->>'kind' = 'mapped'
                      AND (t->>'relid')::oid = 'tb_user'::regclass::oid),
            'post plan does not map tb_user');

\echo 'fk lineage: PASS'

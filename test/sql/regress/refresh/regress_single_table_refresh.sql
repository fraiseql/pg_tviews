-- A TVIEW over one table follows every write to it, whatever its key column is
-- called, and a refresh rewrites only the written row.
--
-- Two TVIEWs created through CREATE TABLE ... AS, keyed on pk_post and pk_user:
-- UPDATE, INSERT and DELETE each reach the right TVIEW, and each base table
-- carries an enabled row trigger and statement-level flush trigger. A third,
-- created by pg_tviews_create over a plain view, checks single-, multi- and
-- all-field updates, an update to NULL, and that refreshing one row leaves the
-- others' tuples (ctid) untouched.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_single_table_refresh.sql
--
-- expect-output: single table refresh: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'single table refresh FAIL: %', what; END IF; END $$;

-- 1. Key column pk_post, TVIEW by CREATE TABLE ... AS.
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title TEXT NOT NULL,
    content TEXT
);
INSERT INTO tb_post (title, content) VALUES ('Original Post', 'Original Content');
CREATE TABLE tv_post AS
SELECT pk_post, id, jsonb_build_object('id', id::text, 'title', title, 'content', content) AS data
FROM tb_post;

SELECT assert_fresh('tv_post', 'pk_post', 'CREATE TABLE tv_post AS');
SELECT must((SELECT count(*) FROM tv_post) = 1
            AND (SELECT data->>'title' FROM tv_post WHERE pk_post = 1) = 'Original Post',
            'tv_post not populated with the original post');

UPDATE tb_post SET title = 'Updated Post', content = 'Updated Content' WHERE pk_post = 1;
SELECT assert_fresh('tv_post', 'pk_post', 'UPDATE tb_post');
SELECT must((SELECT (data->>'title', data->>'content') FROM tv_post WHERE pk_post = 1)
            = ('Updated Post'::text, 'Updated Content'::text),
            'tv_post row 1 not refreshed by UPDATE tb_post');
SELECT must((SELECT count(*) FROM pg_tview_meta WHERE entity = 'post') = 1,
            'no pg_tview_meta row for post');

-- 2. Key column pk_user: the update must land on it too.
CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT NOT NULL,
    email TEXT
);
INSERT INTO tb_user (name, email) VALUES ('Alice', 'alice@example.com');
CREATE TABLE tv_user AS
SELECT pk_user, id, jsonb_build_object('id', id::text, 'name', name, 'email', email) AS data
FROM tb_user;

SELECT assert_fresh('tv_user', 'pk_user', 'CREATE TABLE tv_user AS');
SELECT must((SELECT count(*) FROM tv_user) = 1
            AND (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) = 'Alice',
            'tv_user not populated with Alice');
UPDATE tb_user SET name = 'Alice Updated', email = 'alice.updated@example.com' WHERE pk_user = 1;
SELECT assert_fresh('tv_user', 'pk_user', 'UPDATE tb_user');
SELECT must((SELECT (data->>'name', data->>'email') FROM tv_user WHERE pk_user = 1)
            = ('Alice Updated'::text, 'alice.updated@example.com'::text),
            'tv_user row 1 not refreshed by UPDATE tb_user');
SELECT must((SELECT count(*) FROM pg_tview_meta WHERE entity = 'user') = 1,
            'no pg_tview_meta row for user');

-- 3. INSERT and DELETE.
INSERT INTO tb_post (title, content) VALUES ('New Post', 'New Content');
SELECT assert_fresh('tv_post', 'pk_post', 'INSERT INTO tb_post');
SELECT must((SELECT count(*) FROM tv_post) = 2
            AND (SELECT count(*) FROM tv_post WHERE data->>'title' = 'New Post') = 1,
            'inserted post missing from tv_post');
DELETE FROM tb_post WHERE pk_post = 2;
SELECT assert_fresh('tv_post', 'pk_post', 'DELETE FROM tb_post');
SELECT must((SELECT count(*) FROM tv_post) = 1 AND NOT EXISTS (SELECT 1 FROM tv_post WHERE pk_post = 2),
            'deleted post still in tv_post');

-- 4. Each base table carries an enabled row trigger and statement-level flush trigger.
SELECT must(EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = t AND tgname LIKE 'trg_tview_row_%' AND tgenabled = 'O')
            AND EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = t AND tgname LIKE 'trg_tview_flush_%' AND tgenabled = 'O'),
            'row or flush trigger missing on ' || t)
FROM unnest(ARRAY['tb_post'::regclass, 'tb_user'::regclass]) AS t;

-- 5. pg_tviews_create over a plain view; one row's refresh rewrites only it.
CREATE TABLE tb_article (
    pk_article INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title TEXT NOT NULL,
    body TEXT,
    status TEXT DEFAULT 'draft',
    view_count INTEGER DEFAULT 0
);
INSERT INTO tb_article (title, body, status, view_count) VALUES
    ('First Article', 'First body', 'published', 100),
    ('Second Article', 'Second body', 'draft', 5),
    ('Third Article', 'Third body', 'published', 50);
CREATE VIEW article_prepared AS
SELECT pk_article, id,
       jsonb_build_object('id', id::text, 'title', title, 'body', body,
                          'status', status, 'view_count', view_count) AS data
FROM tb_article;
SELECT pg_tviews_create('tv_article', 'SELECT pk_article, id, data FROM article_prepared');

SELECT assert_fresh('tv_article', 'pk_article', 'pg_tviews_create');
SELECT must((SELECT count(*) FROM tv_article) = 3
            AND (SELECT count(*) FROM tv_article WHERE data->>'status' = 'published') = 2
            AND (SELECT sum((data->>'view_count')::int) FROM tv_article) = 155,
            'tv_article initial population wrong');

-- Every UPDATE writes a new tuple version: a rewritten row has a new ctid.
CREATE TEMP TABLE article_before AS SELECT pk_article, ctid AS tid FROM tv_article;
UPDATE tb_article SET title = 'First Article - Updated' WHERE pk_article = 1;
SELECT assert_fresh('tv_article', 'pk_article', 'an UPDATE of one title');
SELECT must((SELECT data->>'title' FROM tv_article WHERE pk_article = 1) = 'First Article - Updated',
            'article 1 title not refreshed');
SELECT must((SELECT ctid FROM tv_article WHERE pk_article = 1)
            <> (SELECT tid FROM article_before WHERE pk_article = 1),
            'article 1 row not rewritten');
SELECT must((SELECT count(*) FROM tv_article t
             JOIN article_before b ON b.pk_article = t.pk_article AND b.tid = t.ctid
             WHERE t.pk_article <> 1) = 2,
            'refreshing article 1 rewrote other rows');

UPDATE tb_article SET status = 'archived', view_count = 999 WHERE pk_article = 2;
SELECT assert_fresh('tv_article', 'pk_article', 'an UPDATE of two fields');
SELECT must((SELECT (data->>'status', (data->>'view_count')::int) FROM tv_article WHERE pk_article = 2)
            = ('archived'::text, 999),
            'article 2 not (archived, 999)');

UPDATE tb_article SET title = 'New Title', body = 'New Body', status = 'published', view_count = 12345
WHERE pk_article = 3;
SELECT assert_fresh('tv_article', 'pk_article', 'an UPDATE of every field');
SELECT must((SELECT (data->>'title', data->>'body', data->>'status', (data->>'view_count')::int)
             FROM tv_article WHERE pk_article = 3)
            = ('New Title'::text, 'New Body'::text, 'published'::text, 12345),
            'article 3 does not carry all new values');

UPDATE tb_article SET body = NULL WHERE pk_article = 1;
SELECT assert_fresh('tv_article', 'pk_article', 'an UPDATE to NULL');
SELECT must((SELECT data->>'body' IS NULL FROM tv_article WHERE pk_article = 1),
            'article 1 body not NULL');

SELECT must((SELECT count(*) FROM pg_tview_meta) = 3, 'expected exactly three TVIEWs');

\echo 'single table refresh: PASS'

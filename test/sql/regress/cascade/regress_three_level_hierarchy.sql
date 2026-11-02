-- A company -> user -> post hierarchy of TVIEWs, each embedding the TVIEW above
-- it, created through CREATE TABLE ... AS.
--
-- A company update reaches its users and their posts (two embeds deep); a user
-- update reaches only that user's posts; a post update rewrites neither its user
-- nor its company row; inserts embed the current parents; moving a user to
-- another company moves its posts' nested company too. Each plan has one local
-- path, user and post one embed each, and every base table an enabled row and
-- flush trigger.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_three_level_hierarchy.sql
--
-- expect-output: three level hierarchy: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'three level hierarchy FAIL: %', what; END IF; END $$;
CREATE FUNCTION assert_all_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM assert_fresh('tv_company', 'pk_company', label);
    PERFORM assert_fresh('tv_user', 'pk_user', label);
    PERFORM assert_fresh('tv_post', 'pk_post', label);
END $$;

CREATE TABLE tb_company (
    pk_company INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT NOT NULL,
    industry TEXT,
    employee_count INTEGER DEFAULT 0
);
CREATE TABLE tb_user (
    pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_company INTEGER NOT NULL REFERENCES tb_company(pk_company),
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    role TEXT DEFAULT 'member'
);
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_user INTEGER NOT NULL REFERENCES tb_user(pk_user),
    title TEXT NOT NULL,
    content TEXT,
    status TEXT DEFAULT 'draft',
    view_count INTEGER DEFAULT 0,
    created_at TIMESTAMPTZ DEFAULT now()
);
INSERT INTO tb_company (name, industry, employee_count) VALUES
    ('Acme Corp', 'Technology', 150), ('Globex Inc', 'Manufacturing', 500);
INSERT INTO tb_user (fk_company, name, email, role) VALUES
    (1, 'Alice Johnson', 'alice@acme.com', 'admin'),
    (1, 'Bob Smith', 'bob@acme.com', 'member'),
    (1, 'Carol White', 'carol@acme.com', 'member'),
    (2, 'David Brown', 'david@globex.com', 'admin');
INSERT INTO tb_post (fk_user, title, content, status, view_count) VALUES
    (1, 'Welcome to Acme', 'This is our first post', 'published', 100),
    (1, 'Q2 Updates', 'Quarterly updates here', 'published', 50),
    (2, 'Engineering Blog', 'Technical insights', 'draft', 5),
    (3, 'Design Patterns', 'UI/UX best practices', 'published', 75),
    (4, 'Manufacturing News', 'Latest from the floor', 'published', 30);

CREATE TABLE tv_company AS
SELECT pk_company, id,
       jsonb_build_object('id', id::text, 'name', name, 'industry', industry,
                          'employee_count', employee_count) AS data
FROM tb_company;
CREATE TABLE tv_user AS
SELECT u.pk_user, u.id, u.fk_company, c.id AS company_id,
       jsonb_build_object('id', u.id::text, 'name', u.name, 'email', u.email,
                          'role', u.role, 'company', c.data) AS data
FROM tb_user u JOIN tv_company c ON c.pk_company = u.fk_company;
CREATE TABLE tv_post AS
SELECT p.pk_post, p.id, p.fk_user, u.id AS user_id,
       jsonb_build_object('id', p.id::text, 'title', p.title, 'content', p.content,
                          'status', p.status, 'view_count', p.view_count,
                          'created_at', p.created_at, 'author', u.data) AS data
FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user;

-- 1. Initial population, nested two deep.
SELECT assert_all_fresh('creating the hierarchy');
SELECT must((SELECT count(*) FROM tv_company) = 2 AND (SELECT count(*) FROM tv_user) = 4
            AND (SELECT count(*) FROM tv_post) = 5, 'initial row counts not 2/4/5');
SELECT must((SELECT (data->>'title', data->'author'->>'name', data->'author'->'company'->>'name')
             FROM tv_post WHERE pk_post = 1)
            = ('Welcome to Acme'::text, 'Alice Johnson'::text, 'Acme Corp'::text),
            'post 1 nested author/company wrong');

-- 2. A company update reaches its users and their posts.
UPDATE tb_company SET name = 'Acme Corporation' WHERE pk_company = 1;
SELECT assert_all_fresh('an UPDATE of tb_company.name');
SELECT must((SELECT data->>'name' FROM tv_company WHERE pk_company = 1) = 'Acme Corporation',
            'tv_company row 1 not renamed');
SELECT must((SELECT count(*) FROM tv_user WHERE fk_company = 1 AND data->'company'->>'name' = 'Acme Corporation') = 3,
            'rename did not reach all 3 Acme users');
SELECT must((SELECT count(*) FROM tv_post WHERE data->'author'->'company'->>'name' = 'Acme Corporation') = 4,
            'rename did not reach all 4 Acme posts');

-- 3. A user update reaches that user's posts only.
UPDATE tb_user SET name = 'Alice J. Updated' WHERE pk_user = 1;
SELECT assert_all_fresh('an UPDATE of tb_user.name');
SELECT must((SELECT data->>'name' FROM tv_user WHERE pk_user = 1) = 'Alice J. Updated',
            'tv_user row 1 not renamed');
SELECT must((SELECT count(*) FROM tv_post WHERE fk_user = 1 AND data->'author'->>'name' = 'Alice J. Updated') = 2,
            'rename did not reach both of Alice''s posts');
SELECT must((SELECT data->'author'->>'name' FROM tv_post WHERE fk_user = 2) = 'Bob Smith',
            'Bob''s post changed by Alice''s rename');

-- 4. A post update rewrites neither its user nor its company row.
CREATE TEMP TABLE before_post_update AS
SELECT (SELECT ctid FROM tv_user WHERE pk_user = 1) AS user_tid,
       (SELECT ctid FROM tv_company WHERE pk_company = 1) AS company_tid;
UPDATE tb_post SET title = 'Updated Welcome', view_count = 999 WHERE pk_post = 1;
SELECT assert_all_fresh('an UPDATE of tb_post');
SELECT must((SELECT (data->>'title', (data->>'view_count')::int) FROM tv_post WHERE pk_post = 1)
            = ('Updated Welcome'::text, 999), 'post 1 not (Updated Welcome, 999)');
SELECT must((SELECT ctid FROM tv_user WHERE pk_user = 1) = (SELECT user_tid FROM before_post_update)
            AND (SELECT ctid FROM tv_company WHERE pk_company = 1) = (SELECT company_tid FROM before_post_update),
            'a post update rewrote its user or company row');

-- 5. Inserts embed the current parents.
INSERT INTO tb_user (fk_company, name, email, role) VALUES (1, 'Eve Wilson', 'eve@acme.com', 'member');
SELECT assert_all_fresh('an INSERT into tb_user');
SELECT must((SELECT data->'company'->>'name' FROM tv_user WHERE data->>'email' = 'eve@acme.com') = 'Acme Corporation',
            'new user Eve does not embed Acme Corporation');
INSERT INTO tb_post (fk_user, title, content, status) VALUES (5, 'First Post by Eve', 'Hello world', 'published');
SELECT assert_all_fresh('an INSERT into tb_post');
SELECT must((SELECT (data->'author'->>'name', data->'author'->'company'->>'name') FROM tv_post
             WHERE data->>'title' = 'First Post by Eve')
            = ('Eve Wilson'::text, 'Acme Corporation'::text), 'Eve''s post nested author/company wrong');

-- 6. Deleting a post leaves its user.
DELETE FROM tb_post WHERE pk_post = 5;
SELECT assert_all_fresh('a DELETE from tb_post');
SELECT must(NOT EXISTS (SELECT 1 FROM tv_post WHERE pk_post = 5), 'deleted post 5 still in tv_post');
SELECT must(EXISTS (SELECT 1 FROM tv_user WHERE pk_user = 4), 'deleting a post removed its user');

-- 7. Moving a user to another company.
UPDATE tb_user SET fk_company = 2 WHERE pk_user = 2;
SELECT assert_all_fresh('re-pointing tb_user.fk_company');
SELECT must((SELECT data->'company'->>'name' FROM tv_user WHERE pk_user = 2) = 'Globex Inc',
            'moved user Bob does not embed Globex Inc');
SELECT must((SELECT data->'author'->'company'->>'name' FROM tv_post WHERE fk_user = 2) = 'Globex Inc',
            'Bob''s post does not show Globex Inc');

-- 8. Two company fields reach every post of its remaining users.
UPDATE tb_company SET industry = 'Tech & Innovation', employee_count = 200 WHERE pk_company = 1;
SELECT assert_all_fresh('an UPDATE of two tb_company fields');
SELECT must((SELECT ((data->>'employee_count')::int, data->>'industry') FROM tv_company WHERE pk_company = 1)
            = (200, 'Tech & Innovation'::text), 'tv_company row 1 not (200, Tech & Innovation)');
SELECT must((SELECT count(*) FROM tv_post WHERE data->'author'->'company'->>'industry' = 'Tech & Innovation') = 4,
            'industry change did not reach the 4 Acme posts');

-- 9. Plans: one local path each; user embeds tv_company, post embeds tv_user.
SELECT must((SELECT string_agg(format('%s:%s/%s', entity, jsonb_array_length(plan->'paths'),
                                      jsonb_array_length(plan->'embeds')), ',' ORDER BY entity)
             FROM pg_tview_meta) = 'company:1/0,post:1/1,user:1/1',
            'unexpected local paths/embeds per TVIEW');

-- 10. Each base table carries an enabled row trigger and statement-level flush trigger.
SELECT must(EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = t AND tgname LIKE 'trg_tview_row_%' AND tgenabled = 'O')
            AND EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = t AND tgname LIKE 'trg_tview_flush_%' AND tgenabled = 'O'),
            'row or flush trigger missing on ' || t)
FROM unnest(ARRAY['tb_company'::regclass, 'tb_user'::regclass, 'tb_post'::regclass]) AS t;

\echo 'three level hierarchy: PASS'

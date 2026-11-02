-- Writes to the rows a TVIEW aggregates into a JSONB array keep the array exact:
--   * updating one element changes that element only, in place, order kept;
--   * inserting into an empty array, then appending, keeps the ORDER BY order;
--   * deleting the first element, then the last, ends with [] (the FILTER'd
--     COALESCE), not a stale element;
--   * renaming the parent of a 50-element array keeps every element in order, and
--     a write to one parent leaves the other parents' documents alone.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/arrays/regress_array_aggregate_writes.sql
-- expect-output: array_aggregate_writes: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'array aggregate writes FAIL: %', what; END IF; END $$;

-- ── one element updated ─────────────────────────────────────────────────────
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
CREATE TABLE tb_comment (pk_comment int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_post int REFERENCES tb_post (pk_post), author text, text text);
INSERT INTO tb_post (pk_post, title) VALUES (1, 'First Post'), (2, 'Empty Post');
INSERT INTO tb_comment (pk_comment, fk_post, author, text)
    VALUES (1, 1, 'Alice', 'Great post!'), (2, 1, 'Bob', 'Thanks for sharing!');

-- No FILTER: a post without comments would aggregate one all-NULL element, but
-- post 1 has comments; tv_note below covers the empty case.
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.title,
           jsonb_build_object('id', p.id, 'title', p.title,
               'comments', COALESCE(jsonb_agg(jsonb_build_object('id', c.id, 'author', c.author, 'text', c.text)
                                              ORDER BY c.pk_comment), '[]'::jsonb)) AS data
    FROM tb_post p LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
    GROUP BY p.pk_post, p.id, p.title $$);
SELECT assert_fresh('tv_post', 'pk_post', 'pg_tviews_create');
SELECT must((SELECT jsonb_path_query_array(data, '$.comments[*].text') FROM tv_post WHERE pk_post = 1)
            = '["Great post!", "Thanks for sharing!"]', 'initial comments');

UPDATE tb_comment SET text = 'Updated: Great post!' WHERE pk_comment = 1;
SELECT assert_fresh('tv_post', 'pk_post', 'UPDATE tb_comment');
SELECT must((SELECT jsonb_path_query_array(data, '$.comments[*].text') FROM tv_post WHERE pk_post = 1)
            = '["Updated: Great post!", "Thanks for sharing!"]',
            'comments not [updated first, unchanged second]');

-- ── elements inserted into an empty array, then deleted ─────────────────────
CREATE TABLE tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
CREATE TABLE tb_reply (pk_reply int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_note int REFERENCES tb_note (pk_note), author text, text text);
INSERT INTO tb_note (pk_note, title) VALUES (1, 'First Note');
SELECT pg_tviews_create('tv_note', $$
    SELECT n.pk_note, n.id, n.title,
           jsonb_build_object('id', n.id, 'title', n.title,
               'replies', COALESCE(jsonb_agg(jsonb_build_object('id', r.id, 'author', r.author, 'text', r.text)
                                             ORDER BY r.pk_reply) FILTER (WHERE r.pk_reply IS NOT NULL),
                                   '[]'::jsonb)) AS data
    FROM tb_note n LEFT JOIN tb_reply r ON r.fk_note = n.pk_note
    GROUP BY n.pk_note, n.id, n.title $$);
SELECT assert_fresh('tv_note', 'pk_note', 'pg_tviews_create');
SELECT must((SELECT data->'replies' FROM tv_note WHERE pk_note = 1) = '[]', 'replies not empty at first');

INSERT INTO tb_reply (pk_reply, fk_note, author, text) VALUES (1, 1, 'Alice', 'First comment!');
SELECT assert_fresh('tv_note', 'pk_note', 'first INSERT INTO tb_reply');
SELECT must((SELECT jsonb_path_query_array(data, '$.replies[*].author') FROM tv_note WHERE pk_note = 1) = '["Alice"]'
            AND (SELECT data->'replies'->0->>'text' FROM tv_note WHERE pk_note = 1) = 'First comment!',
            'first reply not [Alice: First comment!]');

INSERT INTO tb_reply (pk_reply, fk_note, author, text) VALUES (2, 1, 'Bob', 'Second comment!');
SELECT assert_fresh('tv_note', 'pk_note', 'second INSERT INTO tb_reply');
SELECT must((SELECT jsonb_path_query_array(data, '$.replies[*].author') FROM tv_note WHERE pk_note = 1)
            = '["Alice", "Bob"]', 'replies not [Alice, Bob] after the append');

DELETE FROM tb_reply WHERE pk_reply = 1;
SELECT assert_fresh('tv_note', 'pk_note', 'DELETE of the first reply');
SELECT must((SELECT jsonb_path_query_array(data, '$.replies[*].author') FROM tv_note WHERE pk_note = 1) = '["Bob"]'
            AND (SELECT data->'replies'->0->>'text' FROM tv_note WHERE pk_note = 1) = 'Second comment!',
            'replies not [Bob: Second comment!] after deleting the first');

DELETE FROM tb_reply WHERE pk_reply = 2;
SELECT assert_fresh('tv_note', 'pk_note', 'DELETE of the last reply');
SELECT must((SELECT data->'replies' FROM tv_note WHERE pk_note = 1) = '[]', 'replies not empty after deleting all');

-- ── a parent of 50 elements renamed; another parent untouched ───────────────
CREATE TABLE tb_company (pk_company int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_company int REFERENCES tb_company (pk_company), name text);
INSERT INTO tb_company (pk_company, name) VALUES (1, 'TechCorp');
INSERT INTO tb_user (pk_user, fk_company, name) SELECT i, 1, 'User ' || i FROM generate_series(1, 50) i;
SELECT pg_tviews_create('tv_company', $$
    SELECT c.pk_company, c.id, c.name,
           jsonb_build_object('id', c.id, 'name', c.name,
               'users', COALESCE(jsonb_agg(jsonb_build_object('id', u.id, 'name', u.name) ORDER BY u.pk_user)
                                 FILTER (WHERE u.pk_user IS NOT NULL), '[]'::jsonb)) AS data
    FROM tb_company c LEFT JOIN tb_user u ON u.fk_company = c.pk_company
    GROUP BY c.pk_company, c.id, c.name $$);
SELECT assert_fresh('tv_company', 'pk_company', 'pg_tviews_create');
SELECT must((SELECT jsonb_array_length(data->'users') FROM tv_company WHERE pk_company = 1) = 50
            AND (SELECT data->>'name' FROM tv_company WHERE pk_company = 1) = 'TechCorp',
            'company 1 is not TechCorp with 50 users');

UPDATE tb_company SET name = 'TechCorp Updated' WHERE pk_company = 1;
SELECT assert_fresh('tv_company', 'pk_company', 'UPDATE of the 50-user company');
SELECT must((SELECT jsonb_array_length(data->'users') FROM tv_company WHERE pk_company = 1) = 50
            AND (SELECT data->>'name' FROM tv_company WHERE pk_company = 1) = 'TechCorp Updated'
            AND (SELECT data->'users'->49->>'name' FROM tv_company WHERE pk_company = 1) = 'User 50',
            'company 1 not renamed with its 50 users kept in order');

INSERT INTO tb_company (pk_company, name) VALUES (2, 'SmallCo');
SELECT assert_fresh('tv_company', 'pk_company', 'INSERT INTO tb_company');
INSERT INTO tb_user (pk_user, fk_company, name) VALUES (51, 2, 'Small User 1');
INSERT INTO tb_user (pk_user, fk_company, name) VALUES (52, 2, 'Small User 2');
INSERT INTO tb_user (pk_user, fk_company, name) VALUES (53, 2, 'Small User 3');
SELECT assert_fresh('tv_company', 'pk_company', 'INSERTs of the small company''s users');
SELECT must((SELECT jsonb_array_length(data->'users') FROM tv_company WHERE pk_company = 2) = 3
            AND (SELECT data->>'name' FROM tv_company WHERE pk_company = 2) = 'SmallCo',
            'company 2 is not SmallCo with 3 users');

UPDATE tb_company SET name = 'SmallCo Updated' WHERE pk_company = 2;
SELECT assert_fresh('tv_company', 'pk_company', 'UPDATE of the 3-user company');
SELECT must((SELECT jsonb_array_length(data->'users') FROM tv_company WHERE pk_company = 2) = 3
            AND (SELECT data->>'name' FROM tv_company WHERE pk_company = 2) = 'SmallCo Updated',
            'company 2 not renamed with its 3 users kept');
SELECT must((SELECT jsonb_array_length(data->'users') FROM tv_company WHERE pk_company = 1) = 50
            AND (SELECT data->>'name' FROM tv_company WHERE pk_company = 1) = 'TechCorp Updated',
            'company 1 changed by the update of company 2');

DROP EXTENSION pg_tviews CASCADE;
\echo 'array_aggregate_writes: PASS'

-- Regression test: tviews.registry.references, the relationships a reader composes by.
--
-- A reference is an output column of a TVIEW traced to a column of its root table
-- that a single-column FOREIGN KEY proves to hold the source of another registered
-- TVIEW's identity. Names are never guessed. Each reference column gets a managed
-- (column, identity) index. The registry resolves the parent live, so a parent
-- registered after the child is found, and stays plain SQL.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_references.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS "Blog Space" CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'references FAIL: %', what; END IF; END $$;

-- The registry's references of `entity`, as one jsonb object.
CREATE FUNCTION refs(entity text) RETURNS jsonb LANGUAGE sql AS $$
    SELECT r.references FROM tviews.registry r WHERE r.entity = $1 $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text NOT NULL);
CREATE TABLE tb_category (pk_category int PRIMARY KEY, label text NOT NULL);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_author int NOT NULL REFERENCES tb_user, title text NOT NULL);
CREATE TABLE tb_comment (
    pk_comment  int PRIMARY KEY,
    id          uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_post     int NOT NULL REFERENCES tb_post,
    fk_author   int NOT NULL REFERENCES tb_user,     -- role-named: a user
    reviewed_by int REFERENCES tb_user,              -- not fk_*, nullable
    fk_editor   int,                                 -- no constraint: no reference
    fk_category int REFERENCES tb_category,          -- no TVIEW over tb_category yet
    fk_parent   int REFERENCES tb_comment,           -- to itself
    content     text NOT NULL
);
CREATE TABLE tb_profile (pk_profile int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_user int NOT NULL UNIQUE REFERENCES tb_user, bio text);
CREATE TABLE tb_pair (a int, b int, PRIMARY KEY (a, b));
CREATE TABLE tb_link (pk_link int PRIMARY KEY, a int, b int, FOREIGN KEY (a, b) REFERENCES tb_pair);

INSERT INTO tb_user VALUES (1, DEFAULT, 'alice'), (2, DEFAULT, 'bob');
INSERT INTO tb_category VALUES (1, 'news');
INSERT INTO tb_post VALUES (1, DEFAULT, 1, 'p1'), (2, DEFAULT, 2, 'p2');
INSERT INTO tb_comment VALUES (1, DEFAULT, 1, 2, 1, NULL, 1, NULL, 'c1'),
                              (2, DEFAULT, 1, 1, NULL, 2, NULL, 1, 'c2');

-- The child first: its parents are registered after it.
SELECT tviews.pg_tviews_create('tv_comment', $$
    SELECT c.pk_comment, c.id, c.fk_post, c.fk_author, c.reviewed_by, c.fk_editor,
           c.fk_category, c.fk_parent,
           jsonb_build_object('content', c.content) AS data
    FROM tb_comment c $$);
SELECT must(refs('comment') = '{"fk_parent": {"entity": "comment", "column": "pk_comment",
                "unique": false, "nullable": true, "index": "public.idx_tv_comment_fk_parent_pk_comment",
                "order": ["pk_comment"]}}',
            'before its parents, only the reference to itself: ' || refs('comment')::text);
SELECT must((SELECT plan->'references' @> '[{"column": "fk_category", "table": "public.tb_category",
                                             "key": "pk_category"}]'
             FROM tviews.pg_tview_meta WHERE entity = 'comment'),
            'the plan records a foreign key to a table no TVIEW reads yet');

SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) AS data FROM tb_user u $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_author, jsonb_build_object('title', p.title) AS data
    FROM tb_post p $$);

-- 1. Proven by constraints, role names included; none without one.
SELECT must(refs('comment') = '{
    "fk_post":     {"entity": "post", "column": "pk_post", "unique": false, "nullable": false,
                    "index": "public.idx_tv_comment_fk_post_pk_comment", "order": ["pk_comment"]},
    "fk_author":   {"entity": "user", "column": "pk_user", "unique": false, "nullable": false,
                    "index": "public.idx_tv_comment_fk_author_pk_comment", "order": ["pk_comment"]},
    "reviewed_by": {"entity": "user", "column": "pk_user", "unique": false, "nullable": true,
                    "index": "public.idx_tv_comment_reviewed_by_pk_comment", "order": ["pk_comment"]},
    "fk_parent":   {"entity": "comment", "column": "pk_comment", "unique": false, "nullable": true,
                    "index": "public.idx_tv_comment_fk_parent_pk_comment", "order": ["pk_comment"]}}',
            'comment references: ' || refs('comment')::text);
SELECT must(refs('post') = '{"fk_author": {"entity": "user", "column": "pk_user", "unique": false,
                "nullable": false, "index": "public.idx_tv_post_fk_author_pk_post", "order": ["pk_post"]}}',
            'post references: ' || refs('post')::text);
SELECT must(refs('user') = '{}', 'user has no reference: ' || refs('user')::text);

-- 2. A reference column not named fk_* gets the managed index too, recorded.
SELECT must((SELECT 'public.idx_tv_comment_reviewed_by_pk_comment'::regclass = ANY (managed_indexes)
             FROM tviews.registry WHERE entity = 'comment'),
            'reviewed_by has no managed index');

--    So does one to a table no TVIEW reads: a parent registered later finds it
--    indexed.
CREATE TABLE tb_shelf (pk_shelf int PRIMARY KEY);
CREATE TABLE tb_book (pk_book int PRIMARY KEY, shelf int REFERENCES tb_shelf);
SELECT tviews.pg_tviews_create('tv_book', $$
    SELECT b.pk_book, b.shelf, jsonb_build_object('book', b.pk_book) AS data FROM tb_book b $$);
SELECT must(refs('book') = '{}', 'a reference without a parent TVIEW: ' || refs('book')::text);
SELECT must((SELECT 'public.idx_tv_book_shelf_pk_book'::regclass = ANY (managed_indexes)
             FROM tviews.registry WHERE entity = 'book'),
            'a foreign key column without a parent TVIEW has no managed index');

-- 3. A parent registered later is found; one dropped is no longer referenced.
SELECT tviews.pg_tviews_create('tv_category', $$
    SELECT c.pk_category, jsonb_build_object('label', c.label) AS data FROM tb_category c $$);
SELECT must(refs('comment')->'fk_category'->>'entity' = 'category',
            'a parent registered after the child: ' || refs('comment')::text);
SELECT must(refs('comment')->'fk_category'->>'index' = 'public.idx_tv_comment_fk_category_pk_comment',
            'a parent registered later does not find the column indexed: ' || refs('comment')::text);
SELECT tviews.pg_tviews_drop('tv_category');
SELECT must(NOT refs('comment') ? 'fk_category', 'a dropped parent is still referenced');

-- 4. A unique column is one-to-one.
SELECT tviews.pg_tviews_create('tv_profile', $$
    SELECT p.pk_profile, p.id, p.fk_user, jsonb_build_object('bio', p.bio) AS data
    FROM tb_profile p $$);
SELECT must(refs('profile')->'fk_user'->'unique' = 'true', 'unique: ' || refs('profile')::text);

-- 5. A composite foreign key is no reference.
SELECT tviews.pg_tviews_create('tv_link', $$
    SELECT l.pk_link, l.a, l.b, jsonb_build_object('a', l.a) AS data FROM tb_link l $$);
SELECT must(refs('link') = '{}', 'composite: ' || refs('link')::text);

-- 6. A UNION TVIEW has none, nor is a parent.
SELECT tviews.pg_tviews_create('tv_feed', $$
    SELECT c.pk_comment AS pk_feed, c.fk_post, jsonb_build_object('t', 'c') AS data FROM tb_comment c
    UNION ALL
    SELECT p.pk_post + 1000, p.pk_post, jsonb_build_object('t', 'p') FROM tb_post p $$);
SELECT must(refs('feed') = '{}', 'union: ' || refs('feed')::text);

-- 7. A DISTINCT ON TVIEW over tb_comment is named by fk_post, not pk_comment: a
--    foreign key to tb_comment.pk_comment does not reference it.
SELECT tviews.pg_tviews_create('tv_latest', $$
    SELECT DISTINCT ON (c.fk_post) c.fk_post AS post_key, c.pk_comment AS last_comment,
           jsonb_build_object('content', c.content) AS data
    FROM tb_comment c ORDER BY c.fk_post, c.pk_comment DESC $$);
SELECT must((SELECT count(*) FROM tviews.registry r, jsonb_each(r.references) e
             WHERE e.value->>'entity' = 'latest') = 0,
            'a DISTINCT ON TVIEW named by another column is referenced');

-- 8. Quoted identifiers and another schema.
CREATE SCHEMA "Blog Space";
CREATE TABLE "Blog Space".tb_tag (pk_tag int PRIMARY KEY, "Label" text NOT NULL);
CREATE TABLE "Blog Space".tb_mark (pk_mark int PRIMARY KEY,
                                   "Tag Ref" int NOT NULL REFERENCES "Blog Space".tb_tag);
INSERT INTO "Blog Space".tb_tag VALUES (1, 'x');
INSERT INTO "Blog Space".tb_mark VALUES (1, 1);
SELECT tviews.pg_tviews_create('"Blog Space".tv_tag', $$
    SELECT t.pk_tag, jsonb_build_object('label', t."Label") AS data FROM "Blog Space".tb_tag t $$);
SELECT tviews.pg_tviews_create('"Blog Space".tv_mark', $$
    SELECT m.pk_mark, m."Tag Ref", jsonb_build_object('mark', m.pk_mark) AS data
    FROM "Blog Space".tb_mark m $$);
SELECT must(refs('mark')->'Tag Ref' @> '{"entity": "tag", "column": "pk_tag",
                "index": "\"Blog Space\".\"idx_tv_mark_Tag Ref_pk_mark\""}',
            'quoted: ' || refs('mark')::text);

-- 9. Re-registration keeps them; the registry calls no library function (it reads on
--    a standby).
SELECT tviews.pg_tviews_reregister('comment');
SELECT must(refs('comment') ? 'reviewed_by', 'lost on re-registration');
SELECT must(NOT EXISTS (
    SELECT 1 FROM pg_depend d
    JOIN pg_proc p ON p.oid = d.refobjid AND d.refclassid = 'pg_proc'::regclass
    WHERE d.classid = 'pg_rewrite'::regclass
      AND d.objid IN (SELECT oid FROM pg_rewrite
                      WHERE ev_class IN ('tviews.registry'::regclass,
                                         'tviews.pg_tview_references'::regclass))
      AND p.prolang = (SELECT oid FROM pg_language WHERE lanname = 'c')),
    'the references call a C function');

-- 10. Two TVIEWs named by one key: the parent is the one whose identity has the
--     referenced column's name, else the first by entity. One parent per column.
SELECT tviews.pg_tviews_create('tv_account', $$
    SELECT u.pk_user AS pk_account, jsonb_build_object('name', u.name) AS data FROM tb_user u $$);
SELECT must(refs('post')->'fk_author'->>'entity' = 'user',
            'the parent named like the key is not preferred: ' || refs('post')::text);
SELECT tviews.pg_tviews_create('tv_member', $$
    SELECT u.pk_user AS pk_member, jsonb_build_object('name', u.name) AS data FROM tb_user u $$);
SELECT tviews.pg_tviews_drop('tv_user');
SELECT must(refs('post')->'fk_author'->>'entity' = 'account',
            'without one named like the key, not the first by entity: ' || refs('post')::text);
SELECT must((SELECT count(*) FROM tviews.pg_tview_references
             WHERE entity = 'post' AND column_name = 'fk_author') = 2,
            'the candidates are not both listed');
SELECT must((SELECT array_agg(parent) FROM tviews.pg_tview_references
             WHERE entity = 'post' AND column_name = 'fk_author' AND chosen)
            = ARRAY[refs('post')->'fk_author'->>'entity'],
            'not exactly one candidate chosen, the one the registry names');
-- A parent whose identity is unknown (a row awaiting re-registration) never wins: its
-- `preferred` is NULL, which must not sort before false.
UPDATE tviews.pg_tview_meta SET identity = NULL WHERE entity = 'account';
SELECT must(refs('post')->'fk_author'->>'entity' = 'member',
            'a parent without an identity was chosen: ' || refs('post')::text);
UPDATE tviews.pg_tview_meta m SET identity = r.identity
FROM (SELECT jsonb_build_object('kind', 'pk', 'columns',
             jsonb_build_array(jsonb_build_object('name', 'pk_account'))) AS identity) r
WHERE m.entity = 'account';

-- 11. registry.typename is the type name the report and the change feed use, so a
--     reader names a collection (`Comment.fk_post`) from the registry alone: the
--     typename option, else the PascalCase of the entity.
CREATE TABLE tb_blog_entry (pk_blog_entry int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL);
CREATE TABLE "tb_été" (pk_été int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       title text NOT NULL);
SELECT tviews.pg_tviews_create('tv_blog_entry', $$
    SELECT e.pk_blog_entry, e.id, jsonb_build_object('title', e.title) AS data FROM tb_blog_entry e $$);
SELECT tviews.pg_tviews_create('"tv_été"', $$
    SELECT e.pk_été, e.id, jsonb_build_object('title', e.title) AS data FROM "tb_été" e $$);
SELECT tviews.pg_tviews_create('tv_note', $$
    SELECT e.pk_blog_entry AS pk_note, e.id, jsonb_build_object('title', e.title) AS data
    FROM tb_blog_entry e $$, '{"typename": "Article"}');
BEGIN;
INSERT INTO tb_blog_entry (pk_blog_entry, title) VALUES (1, 'a');
INSERT INTO "tb_été" (pk_été, title) VALUES (1, 'a');
SELECT tviews.pg_tviews_flush_and_report(include_data => false) AS r \gset
SELECT must((SELECT array_agg(DISTINCT e->>'__typename' ORDER BY e->>'__typename')
             FROM jsonb_array_elements((:'r'::jsonb)->'updated') e)
            = (SELECT array_agg(typename ORDER BY typename) FROM tviews.registry
               WHERE entity IN ('blog_entry', 'été', 'note')),
            'registry.typename is not what the report names: ' || :'r');
COMMIT;
SELECT must((SELECT typename FROM tviews.registry WHERE entity = 'blog_entry') = 'BlogEntry'
            AND (SELECT typename FROM tviews.registry WHERE entity = 'note') = 'Article'
            AND (SELECT typename FROM tviews.registry WHERE entity = 'été') = 'été',
            'typename: PascalCase of the entity (ASCII letters only), else the option');

\echo 'references: PASS'

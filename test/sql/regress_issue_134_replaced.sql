-- Regression test (#134): a definition change that keeps the columns is `replaced`.
--
-- Rebuilding a TVIEW for every change to its definition would drop and recompute
-- the table, and is refused as soon as anything depends on it. When the new
-- definition produces the same columns (and group_keys), create_or_replace()
-- replaces the backing view and reconciles the rows in place instead: only rows
-- that change are written, the table keeps its identity, indexes, privileges and
-- dependents, and the TVIEWs that read its view are re-registered and reconciled.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_134_replaced.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP ROLE IF EXISTS regress_134r_owner;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#134 replaced FAIL: %', what; END IF; END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (
    pk_post int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_user int NOT NULL REFERENCES tb_user,
    title   text
);
CREATE TABLE tb_feed (
    pk_feed int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_post int NOT NULL REFERENCES tb_post
);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 1, 'p2'), (3, 2, 'p3');
INSERT INTO tb_feed (pk_feed, fk_post) VALUES (1, 1), (2, 2), (3, 3);

SELECT tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title) AS data
    FROM tb_post p $$);
-- A TVIEW that embeds tv_post through its view, and a plain view over its table:
-- both depend on it, which rules out a rebuild.
SELECT tviews.pg_tviews_create_or_replace('tv_feed', $$
    SELECT f.pk_feed, f.id, f.fk_post, jsonb_build_object('post', v.data) AS data
    FROM tb_feed f JOIN v_post v ON v.pk_post = f.fk_post $$);
CREATE VIEW post_titles AS SELECT pk_post, data->>'title' AS title FROM tv_post;
CREATE INDEX tv_post_title_idx ON tv_post ((data->>'title'));
COMMENT ON TABLE tv_post IS 'posts';

CREATE FUNCTION assert_fresh(step text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE entity text; d bigint;
BEGIN
    FOR entity IN SELECT m.entity FROM tviews.pg_tview_meta m LOOP
        EXECUTE format(
            'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM v_%1$s)
                         UNION ALL (SELECT pk_%1$s, data FROM v_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d',
            entity) INTO d;
        PERFORM must(d = 0, format('tv_%s diverges from v_%s after %s', entity, entity, step));
    END LOOP;
END $$;

CREATE TEMP TABLE before AS
    SELECT 'tv_post'::regclass::oid AS tv, pk_post, xmin::text AS row_xmin FROM tv_post;

-- 1. A change to one row's document: replaced, that row only.
SELECT must(tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', CASE WHEN p.pk_post = 1 THEN upper(p.title)
                                            ELSE p.title END) AS data
    FROM tb_post p $$) = 'replaced', 'same columns');
SELECT assert_fresh('replaced');
SELECT must((SELECT data->>'title' FROM tv_post WHERE pk_post = 1) = 'P1', 'row 1 reconciled');
SELECT must((SELECT tv FROM before LIMIT 1) = 'tv_post'::regclass::oid, 'same table');
SELECT must((SELECT xmin::text FROM tv_post WHERE pk_post = 2)
            = (SELECT row_xmin FROM before WHERE pk_post = 2), 'unchanged row 2 not written');
SELECT must((SELECT data->'post'->>'title' FROM tv_feed WHERE pk_feed = 1) = 'P1',
            'TVIEW embedding tv_post followed the change');
SELECT must(to_regclass('post_titles') IS NOT NULL AND to_regclass('tv_post_title_idx') IS NOT NULL
            AND obj_description('tv_post'::regclass, 'pg_class') = 'posts',
            'dependents, indexes and comment kept');

-- 2. A filter: rows leave, then come back.
SELECT must(tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', CASE WHEN p.pk_post = 1 THEN upper(p.title)
                                            ELSE p.title END) AS data
    FROM tb_post p WHERE p.pk_post <> 3 $$) = 'replaced', 'filter');
SELECT assert_fresh('filter');
SELECT must(NOT EXISTS (SELECT 1 FROM tv_post WHERE pk_post = 3), 'filtered row deleted');
SELECT must(tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title) AS data
    FROM tb_post p $$) = 'replaced', 'filter removed');
SELECT assert_fresh('filter removed');
SELECT must(EXISTS (SELECT 1 FROM tv_post WHERE pk_post = 3), 'row back');

-- 3. A new source table: its triggers are installed, and removed again.
SELECT must(tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$) = 'replaced', 'join added');
UPDATE tb_user SET name = 'alicia' WHERE pk_user = 1;
SELECT assert_fresh('author update after join added');
SELECT must((SELECT data->>'author' FROM tv_post WHERE pk_post = 1) = 'alicia',
            'tv_post follows the newly read table');
SELECT must(tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title) AS data
    FROM tb_post p $$) = 'replaced', 'join removed');
SELECT must(NOT EXISTS (
    SELECT 1 FROM pg_trigger t JOIN pg_proc f ON f.oid = t.tgfoid
    WHERE t.tgrelid = 'tb_user'::regclass AND f.pronamespace = 'tviews'::regnamespace
      AND t.tgargs = convert_to('post', 'UTF8') || '\x00'::bytea),
    'tv_post''s triggers on tb_user removed');
SELECT assert_fresh('join removed');

-- 4. The same columns with new storage: replaced, and the storage applied.
SELECT must(tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', lower(p.title)) AS data
    FROM tb_post p $$, '{"fillfactor": 80}') = 'replaced', 'query and storage');
SELECT must((SELECT (options->>'fillfactor')::int FROM tviews.registry WHERE entity = 'post') = 80,
            'storage applied with the replace');
SELECT assert_fresh('query and storage');

-- 5. Other columns still rebuild, and a rebuild is refused here (dependents).
SELECT must((SELECT count(*) FROM tviews.pg_tview_meta WHERE entity = 'post') = 1, 'registered');
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create_or_replace('tv_post', $x$
        SELECT p.pk_post, p.id, p.fk_user, p.title, jsonb_build_object('t', p.title) AS data
        FROM tb_post p $x$);
    RAISE EXCEPTION '#134 replaced FAIL: a column change was not refused';
EXCEPTION WHEN OTHERS THEN
    PERFORM must(SQLERRM LIKE '%post_titles%', 'rebuild refusal names the dependent view');
END $$;

-- 6. Base-table writes keep working after all this.
UPDATE tb_post SET title = 'Hello' WHERE pk_post = 2;
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (4, 2, 'p4');
INSERT INTO tb_feed (pk_feed, fk_post) VALUES (4, 4);
SELECT assert_fresh('writes after replaces');

-- 7. A TVIEW that reads v_post is re-registered and reconciled with it: its
--    triggers follow the tables v_post reads now.
ALTER TABLE tb_post ADD COLUMN subject text;
UPDATE tb_post SET subject = 's' || pk_post;
CREATE TABLE tb_headline (
    pk_headline int PRIMARY KEY,
    id          uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_post     int NOT NULL REFERENCES tb_post
);
INSERT INTO tb_headline (pk_headline, fk_post) VALUES (1, 1), (2, 2);
SELECT must(tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$) = 'replaced', 'join added again');
SELECT tviews.pg_tviews_create_or_replace('tv_headline', $$
    SELECT h.pk_headline, h.id, h.fk_post, jsonb_build_object('title', v.data->>'title') AS data
    FROM tb_headline h JOIN v_post v ON v.pk_post = h.fk_post $$);
-- tv_headline reads tb_user through v_post: refreshing tv_post (which maps it)
-- refreshes tv_headline (ADR 0157 `propagated`).
CREATE FUNCTION reads_users(entity text) RETURNS boolean LANGUAGE sql AS $$
    SELECT (SELECT cascade_kinds->>'tb_user' FROM tviews.registry r WHERE r.entity = $1)
           IS NOT DISTINCT FROM 'propagated' $$;
SELECT must(reads_users('headline'), 'tv_headline reads tb_user through v_post');
SELECT must(tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.subject) AS data
    FROM tb_post p $$) = 'replaced', 'title from subject, join removed');
SELECT assert_fresh('title from subject');
SELECT must(NOT reads_users('headline'), 'tv_headline no longer reads tb_user');
UPDATE tb_post SET subject = 'new subject' WHERE pk_post = 1;
SELECT assert_fresh('subject update');
SELECT must((SELECT data->>'title' FROM tv_headline WHERE pk_headline = 1) = 'new subject',
            'tv_headline follows the column v_post reads now');

-- 8. Rows the new definition drops are deleted first, so a unique index holds
--    throughout: row 2 takes the title row 3 gives up.
CREATE UNIQUE INDEX tv_post_title_key ON tv_post ((data->>'title'));
SELECT must(tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', CASE WHEN p.pk_post = 2 THEN 's3' ELSE p.subject END) AS data
    FROM tb_post p WHERE p.pk_post <> 3 $$) = 'replaced', 'unique value moves between rows');
SELECT assert_fresh('unique value moved');
DROP INDEX tv_post_title_key;

-- 9. The DISTINCT ON key decides the table's key: a change to it rebuilds.
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), label text);
INSERT INTO tb_tag (pk_tag, label) VALUES (1, 'a'), (2, 'b');
SELECT tviews.pg_tviews_create_or_replace('tv_tag', $$
    SELECT t.pk_tag, t.id, jsonb_build_object('label', t.label) AS data FROM tb_tag t $$);
SELECT must(tviews.pg_tviews_create_or_replace('tv_tag', $$
    SELECT DISTINCT ON (t.pk_tag) t.pk_tag, t.id, jsonb_build_object('label', t.label) AS data
    FROM tb_tag t ORDER BY t.pk_tag $$) = 'replaced', 'DISTINCT ON the same key is replaced');
SELECT must(tviews.pg_tviews_create_or_replace('tv_tag', $$
    SELECT DISTINCT ON (t.id) t.pk_tag, t.id, jsonb_build_object('label', t.label) AS data
    FROM tb_tag t ORDER BY t.id $$) = 'rebuilt', 'DISTINCT ON another key rebuilds');
SELECT must((SELECT array_agg(a.attname::text) FROM pg_index i
             JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY (i.indkey)
             WHERE i.indrelid = 'tv_tag'::regclass AND i.indisprimary) = '{id}',
            'rebuilt table keyed on the DISTINCT ON key');
SELECT assert_fresh('DISTINCT ON added');

-- 10. An owner with only SELECT and TRIGGER on the base table can replace: the
--     base table is locked as its owner.
CREATE ROLE regress_134r_owner;
GRANT CREATE ON SCHEMA public TO regress_134r_owner;
CREATE TABLE tb_label (pk_label int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_label (pk_label, name) VALUES (1, 'l1');
GRANT SELECT, TRIGGER ON tb_label TO regress_134r_owner;
SET ROLE regress_134r_owner;
SELECT tviews.pg_tviews_create_or_replace('tv_label', $$
    SELECT l.pk_label, l.id, jsonb_build_object('name', l.name) AS data FROM tb_label l $$);
SELECT must(tviews.pg_tviews_create_or_replace('tv_label', $$
    SELECT l.pk_label, l.id, jsonb_build_object('name', upper(l.name)) AS data FROM tb_label l $$)
    = 'replaced', 'replaced by an owner without write access to the base table');
RESET ROLE;
SELECT assert_fresh('replaced by a non-superuser owner');

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tv_label;
REVOKE ALL ON SCHEMA public FROM regress_134r_owner;
DROP OWNED BY regress_134r_owner;
DROP ROLE regress_134r_owner;

SELECT 'issue #134 replaced: PASS' AS result;
-- expect-output: issue #134 replaced: PASS

-- Regression test for issue #181 (prerequisite): pg_tviews finds a TVIEW's
-- backing view by OID after it is created, never by the name v_<entity>. With the
-- backing view renamed by hand, re-registration, a column rename and a rebuild by
-- pg_tviews_create_or_replace() keep working.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_181_view_by_oid.sql
--
-- expect-output: issue #181 view by OID: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'alice'), (2, 'bob');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');

SELECT tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.name) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
SELECT format('%s', view) AS post_view FROM tviews.registry WHERE entity = 'post' \gset
ALTER VIEW :post_view RENAME TO post_def;
GRANT SELECT ON tv_post TO PUBLIC;

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE d bigint;
BEGIN
    EXECUTE format(
        'SELECT count(*) FROM ((SELECT pk_post, data FROM tv_post EXCEPT SELECT pk_post, data FROM %1$s)
                     UNION ALL (SELECT pk_post, data FROM %1$s EXCEPT SELECT pk_post, data FROM tv_post)) d',
        (SELECT view FROM tviews.registry WHERE entity = 'post')) INTO d;
    IF d <> 0 THEN
        RAISE EXCEPTION '#181 FAIL: tv_post stale after %', label;
    END IF;
END $$;

-- Re-registration reads the view it has.
DO $$ BEGIN
    PERFORM tviews.pg_tviews_reregister('post');
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#181 FAIL: re-registration after the view was renamed: %', SQLERRM;
END $$;
UPDATE tb_user SET name = 'alice2' WHERE pk_user = 1;
SELECT check_fresh('a user rename after re-registration');

-- A column rename re-derives the metadata from the view it has.
DO $$ BEGIN
    ALTER TABLE tb_user RENAME COLUMN name TO full_name;
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#181 FAIL: a column rename after the view was renamed: %', SQLERRM;
END $$;
UPDATE tb_user SET full_name = 'bob2' WHERE pk_user = 2;
SELECT check_fresh('a renamed column update');

-- A rebuild restores the grants on the table it rebuilds.
DO $$ BEGIN
    PERFORM tviews.pg_tviews_create_or_replace('tv_post', $q$
        SELECT p.pk_post, p.id, p.fk_user, u.pk_user AS author_pk,
               jsonb_build_object('title', p.title, 'author', u.full_name) AS data
        FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $q$);
EXCEPTION WHEN OTHERS THEN
    RAISE EXCEPTION '#181 FAIL: a rebuild after the view was renamed: %', SQLERRM;
END $$;
DO $$ BEGIN
    IF NOT has_table_privilege('public', 'tv_post', 'SELECT') THEN
        RAISE EXCEPTION '#181 FAIL: the rebuild lost the grant on tv_post';
    END IF;
END $$;
UPDATE tb_post SET title = 'p1b' WHERE pk_post = 1;
SELECT check_fresh('a post update after the rebuild');

\echo 'issue #181 view by OID: PASS'

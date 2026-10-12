-- Regression test (#224): the statements pg_tviews generates are planned once
-- per backend, and stay correct across DDL.
--
-- Each write ran ~10 generated statements, nine of them planned every time. They
-- now run from plans kept for the backend: a second identical write prepares
-- nothing (plans_prepared in pg_tviews_queue_stats()), and a TVIEW or base table
-- changed between writes in the same session is still refreshed right.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_kept_plans.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text, bio text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user SELECT g, gen_random_uuid(), 'u' || g, 'b' || g FROM generate_series(1, 5) g;
INSERT INTO tb_post SELECT g, gen_random_uuid(), 1 + g % 5, 't' || g FROM generate_series(1, 20) g;

SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name, 'bio', bio) AS data FROM tb_user $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', jsonb_build_object('name', u.name, 'bio', u.bio)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);

CREATE FUNCTION prepared() RETURNS bigint LANGUAGE sql AS
    $$ SELECT (tviews.pg_tviews_queue_stats()->>'plans_prepared')::bigint $$;
CREATE FUNCTION assert_fresh(step text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE entity text; d bigint;
BEGIN
    FOREACH entity IN ARRAY ARRAY['user', 'post'] LOOP
        EXECUTE format(
            'SELECT count(*) FROM ((SELECT pk_%1$s, data FROM tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tviews.public__tv_%1$s)
                         UNION ALL (SELECT pk_%1$s, data FROM tviews.public__tv_%1$s
                                    EXCEPT SELECT pk_%1$s, data FROM tv_%1$s)) d', entity) INTO d;
        IF d <> 0 THEN
            RAISE EXCEPTION 'kept plans FAIL after %: tv_% diverges (% rows)', step, entity, d;
        END IF;
    END LOOP;
END $$;
CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'kept plans FAIL: %', what; END IF; END $$;

-- Each kind of write once, to prepare what it runs.
UPDATE tb_user SET bio = 'warm' WHERE pk_user = 1;
UPDATE tb_user SET name = 'warm' WHERE pk_user = 1;
INSERT INTO tb_post VALUES (100, default, 1, 'warm');
UPDATE tb_post SET title = 'warm2' WHERE pk_post = 100;
DELETE FROM tb_post WHERE pk_post = 100;
SELECT prepared() AS warm \gset
SELECT must(:warm > 0, 'the writes prepared their statements');

-- 1. The same writes again: nothing prepared.
UPDATE tb_user SET bio = 'again' WHERE pk_user = 2;
UPDATE tb_user SET name = 'again' WHERE pk_user = 2;
INSERT INTO tb_post VALUES (101, default, 2, 'again');
UPDATE tb_post SET title = 'again2' WHERE pk_post = 101;
DELETE FROM tb_post WHERE pk_post = 101;
SELECT must(prepared() = :warm, format('second writes prepared %s statements', prepared() - :warm));
SELECT assert_fresh('second writes');

-- 2. A base table altered between writes.
ALTER TABLE tb_user ADD COLUMN extra int;
ALTER TABLE tb_user ALTER COLUMN extra TYPE bigint;
ALTER TABLE tb_user DROP COLUMN extra;
UPDATE tb_user SET bio = 'after alter' WHERE pk_user = 3;
UPDATE tb_user SET name = 'after alter' WHERE pk_user = 3;
SELECT assert_fresh('base table altered');

-- 3. A TVIEW replaced between writes (another definition, in place and rebuilt).
SELECT tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', upper(p.title), 'author', jsonb_build_object('name', u.name, 'bio', u.bio)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
UPDATE tb_post SET title = 'replaced' WHERE pk_post = 1;
UPDATE tb_user SET bio = 'replaced' WHERE pk_user = 4;
SELECT assert_fresh('replaced in place');
SELECT tviews.pg_tviews_create_or_replace('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, p.title AS title_col,
           jsonb_build_object('title', p.title, 'author', jsonb_build_object('name', u.name, 'bio', u.bio)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
UPDATE tb_post SET title = 'rebuilt' WHERE pk_post = 2;
UPDATE tb_user SET bio = 'rebuilt' WHERE pk_user = 5;
SELECT assert_fresh('rebuilt');

-- 4. Dropped and created again under the same name.
SELECT tviews.pg_tviews_drop('tv_post');
DROP TABLE tb_post;
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text, body text);
INSERT INTO tb_post SELECT g, gen_random_uuid(), 1 + g % 5, 't' || g, 'b' FROM generate_series(1, 10) g;
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'body', p.body, 'author', jsonb_build_object('name', u.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
UPDATE tb_post SET title = 'recreated', body = 'x' WHERE pk_post = 3;
UPDATE tb_user SET name = 'recreated' WHERE pk_user = 1;
INSERT INTO tb_post VALUES (50, default, 2, 'n', 'n');
DELETE FROM tb_post WHERE pk_post = 4;
SELECT assert_fresh('dropped and created again');

-- 5. Another session's DDL (a reconnect stands in for the other backend's
--    invalidation, which the cross-backend tests cover): still right.
\c
SET client_min_messages TO WARNING;
UPDATE tb_user SET bio = 'new session' WHERE pk_user = 2;
SELECT assert_fresh('new session');

SELECT 'kept plans: PASS' AS result;
-- expect-output: kept plans: PASS

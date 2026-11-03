-- The propagation plan stored in pg_tview_meta.plan (ADR 0203) is versioned
-- (version 3) and lists its tables and paths as arrays. A user object built from
-- a LEFT JOIN through fk_user is a mapped table, not an embed of another TVIEW,
-- and the flush follows that mapping: renaming the user refreshes the post.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_stored_plan_shape.sql
-- expect-output: stored_plan_shape: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'stored plan shape FAIL: %', what; END IF; END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int REFERENCES tb_user (pk_user), title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'First Post');

SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', jsonb_build_object('name', u.name)) AS data
    FROM tb_post p LEFT JOIN tb_user u ON p.fk_user = u.pk_user $$);
SELECT assert_fresh('tv_post', 'pk_post', 'pg_tviews_create');

SELECT must((SELECT count(*) FROM tviews.pg_tview_meta WHERE entity = 'post') = 1, 'no pg_tview_meta row for post');
SELECT must((SELECT (plan->>'version')::int = 3
                    AND jsonb_typeof(plan->'tables') = 'array'
                    AND jsonb_typeof(plan->'paths') = 'array'
             FROM tviews.pg_tview_meta WHERE entity = 'post'),
            'the post plan is not a version 3 plan with tables and paths');
SELECT must((SELECT jsonb_array_length(plan->'embeds') FROM tviews.pg_tview_meta WHERE entity = 'post') = 0,
            'the post plan embeds a TVIEW');
SELECT must((SELECT e->>'kind' FROM tviews.pg_tview_meta, jsonb_array_elements(plan->'tables') e
             WHERE entity = 'post' AND e->>'table' LIKE '%tb_user') = 'mapped',
            'tb_user is not mapped in the post plan');

UPDATE tb_user SET name = 'Alice B.' WHERE pk_user = 1;
SELECT assert_fresh('tv_post', 'pk_post', 'UPDATE tb_user');
SELECT must((SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 1) = 'Alice B.',
            'tv_post author not refreshed by UPDATE tb_user');

DROP EXTENSION pg_tviews CASCADE;
\echo 'stored_plan_shape: PASS'

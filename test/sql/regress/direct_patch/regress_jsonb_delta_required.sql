-- jsonb_delta is a required dependency (docs/getting-started/installation.md): one
-- mode, in which writes always patch documents in place.
--
-- CREATE EXTENSION pg_tviews without jsonb_delta fails (PostgreSQL's "required
-- extension is not installed"), and installs it with CASCADE; jsonb_delta cannot be
-- dropped from under pg_tviews; a fan-out patch is written on every install.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/direct_patch/regress_jsonb_delta_required.sql
-- expect-output: jsonb_delta_required: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION 'jsonb_delta required FAIL: %', msg; END IF; END $$;

-- Without jsonb_delta: refused.
\set ON_ERROR_STOP off
CREATE EXTENSION pg_tviews;
\set ON_ERROR_STOP on
SELECT must(:'LAST_ERROR_SQLSTATE' = '42704', 'CREATE EXTENSION pg_tviews without jsonb_delta gave '
            || :'LAST_ERROR_SQLSTATE');
SELECT must(NOT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_tviews'),
            'pg_tviews installed without jsonb_delta');

-- CASCADE installs it.
CREATE EXTENSION pg_tviews CASCADE;
SELECT must(EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'jsonb_delta'),
            'CASCADE did not install jsonb_delta');

-- It cannot be dropped from under pg_tviews.
\set ON_ERROR_STOP off
DROP EXTENSION jsonb_delta;
\set ON_ERROR_STOP on
SELECT must(:'LAST_ERROR_SQLSTATE' = '2BP01', 'DROP EXTENSION jsonb_delta gave '
            || :'LAST_ERROR_SQLSTATE');

-- A fan-out patch: renaming a user writes the name into its posts' documents.
CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user VALUES (1, DEFAULT, 'ann');
INSERT INTO tb_post VALUES (1, DEFAULT, 1, 'p1'), (2, DEFAULT, 1, 'p2');
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', jsonb_build_object('name', u.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$) \g /dev/null
BEGIN;
UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
SELECT must((tviews.pg_tviews_show_queue()->>'direct_patches_applied')::int = 2
            AND (tviews.pg_tviews_show_queue()->>'view_recomputes')::int = 0,
            'the rename was not written as a fan-out patch: ' || tviews.pg_tviews_show_queue()::text);
COMMIT;
SELECT must((SELECT count(*) FROM tv_post WHERE data #>> '{author,name}' = 'grace') = 2,
            'tv_post does not show the new name');

\echo 'jsonb_delta_required: PASS'

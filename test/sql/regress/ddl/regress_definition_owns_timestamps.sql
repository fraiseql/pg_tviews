-- A TVIEW's table holds its definition's columns and no other (fraiseql#1818):
-- pg_tviews adds no created_at / updated_at of its own, so a definition projects
-- the domain's dates under those names, and they hold the base rows' values
-- through every refresh.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_definition_owns_timestamps.sql
--
-- expect-output: definition owns timestamps: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, msg text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF ok IS NOT TRUE THEN RAISE EXCEPTION 'definition owns timestamps FAIL: %', msg; END IF;
END $$;

CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text,
                      created_at timestamptz NOT NULL,
                      updated_at timestamptz NOT NULL);
INSERT INTO tb_post VALUES (1, DEFAULT, 'a', '2020-01-01 00:00+00', '2021-01-01 00:00+00'),
                           (2, DEFAULT, 'b', '2020-06-01 00:00+00', '2020-06-01 00:00+00');

SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT pk_post, id, created_at, updated_at,
           jsonb_build_object('title', title, 'createdAt', created_at) AS data
    FROM tb_post $$);
SELECT must((SELECT created_at = '2020-01-01 00:00+00' AND updated_at = '2021-01-01 00:00+00'
             FROM tv_post WHERE pk_post = 1),
            'created_at and updated_at hold the base row''s values');

UPDATE tb_post SET title = 'a!', updated_at = '2022-01-01 00:00+00' WHERE pk_post = 1;
SELECT must((SELECT created_at = '2020-01-01 00:00+00' AND updated_at = '2022-01-01 00:00+00'
             FROM tv_post WHERE pk_post = 1),
            'a refresh writes the definition''s updated_at, not the write time');
SELECT assert_fresh('tv_post', 'pk_post', 'an UPDATE');

-- A TVIEW that projects neither has neither.
SELECT tviews.pg_tviews_create('tv_title', $$
    SELECT pk_post AS pk_title, id, jsonb_build_object('title', title) AS data FROM tb_post $$);
SELECT must(NOT EXISTS (SELECT 1 FROM pg_attribute WHERE attrelid = 'tv_title'::regclass
                        AND attname IN ('created_at', 'updated_at') AND NOT attisdropped),
            'tv_title has no created_at / updated_at');

\echo 'definition owns timestamps: PASS'

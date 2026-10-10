-- Regression test (#92): the internal diagnostics are still available on request.
-- With pg_tviews.log_level = 'debug' the transcript must contain the diagnostics.
--
-- expect-output: create_tview
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_debug_diagnostics.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title TEXT
);

SET client_min_messages TO NOTICE;
SET pg_tviews.log_level = 'debug';

CREATE TABLE tv_post AS
SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;

SET client_min_messages TO WARNING;
DROP EXTENSION pg_tviews CASCADE;

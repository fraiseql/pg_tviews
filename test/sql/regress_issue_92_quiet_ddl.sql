-- Regression test (#92): normal DDL (CTAS, pg_tviews_create, refresh) must not print
-- internal diagnostics (EVENT TRIGGER banners, DEBUG: lines, spi_run_ddl) at the default
-- client_min_messages. The runner greps the psql transcript of files marked expect-quiet.
--
-- expect-quiet
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_92_quiet_ddl.sql

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
INSERT INTO tb_post (title) VALUES ('one');
CREATE TABLE tb_tag (
    pk_tag INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT
);

SET client_min_messages TO NOTICE;

-- Plain tv_* table with a column list: never converted, never chatty.
CREATE TABLE tv_plain (a int);

CREATE TABLE tv_post AS
SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;

SELECT pg_tviews_create('tag',
  'SELECT pk_tag, id, jsonb_build_object(''name'', name) AS data FROM tb_tag');

UPDATE tb_post SET title = 'two';

SET client_min_messages TO WARNING;
DROP TABLE tv_plain;
DROP EXTENSION pg_tviews CASCADE;

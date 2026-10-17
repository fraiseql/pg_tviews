-- Regression test (#251): a write needs no USAGE on the schemas of TVIEWs it
-- cannot reach.
--
-- A write loads the catalog rows of every TVIEW, and their collections were
-- matched through `tviews.pg_tview_references`, which resolved each referenced
-- table by name (`to_regclass`). A schema-qualified name lookup checks USAGE on
-- the schema, so a writer without USAGE on the schema of a TVIEW with a
-- reference got "permission denied for schema" and its write rolled back.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_issue_251_schema_usage.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP SCHEMA IF EXISTS regress_251_a CASCADE;
DROP SCHEMA IF EXISTS regress_251_b CASCADE;
DROP ROLE IF EXISTS regress_251_writer;
CREATE ROLE regress_251_writer;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE SCHEMA regress_251_a;
CREATE SCHEMA regress_251_b;
CREATE TABLE regress_251_a.tb_alpha (
    pk_alpha bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    v        text
);
INSERT INTO regress_251_a.tb_alpha (v) VALUES ('a');
CREATE TABLE regress_251_a.tv_alpha AS
SELECT a.pk_alpha, a.id, jsonb_build_object('v', a.v) AS data FROM regress_251_a.tb_alpha a;

-- A TVIEW with a reference (fk_beta), in a schema the writer cannot use.
CREATE TABLE regress_251_b.tb_beta (
    pk_beta bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    v       text
);
CREATE TABLE regress_251_b.tb_gamma (
    pk_gamma bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id       uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_beta  bigint REFERENCES regress_251_b.tb_beta,
    v        text
);
CREATE TABLE regress_251_b.tv_beta AS
SELECT b.pk_beta, b.id, jsonb_build_object('v', b.v) AS data FROM regress_251_b.tb_beta b;
CREATE TABLE regress_251_b.tv_gamma AS
SELECT g.pk_gamma, g.id, g.fk_beta, jsonb_build_object('v', g.v) AS data
FROM regress_251_b.tb_gamma g;

-- The reference is still published, for a role that can see it.
DO $$
BEGIN
    IF (SELECT count(*) FROM tviews.pg_tview_references
        WHERE entity = 'gamma' AND parent = 'beta' AND chosen) <> 1 THEN
        RAISE EXCEPTION 'tv_gamma.fk_beta is not published as a reference to beta';
    END IF;
END $$;

GRANT USAGE ON SCHEMA regress_251_a, tviews TO regress_251_writer;
GRANT SELECT, UPDATE ON regress_251_a.tb_alpha TO regress_251_writer;
GRANT SELECT ON regress_251_a.tv_alpha TO regress_251_writer;

SET ROLE regress_251_writer;
UPDATE regress_251_a.tb_alpha SET v = 'b';
DO $$
BEGIN
    IF (SELECT data->>'v' FROM regress_251_a.tv_alpha) IS DISTINCT FROM 'b' THEN
        RAISE EXCEPTION 'tv_alpha was not refreshed: %',
            (SELECT data FROM regress_251_a.tv_alpha);
    END IF;
END $$;
RESET ROLE;

DROP SCHEMA regress_251_a CASCADE;
DROP SCHEMA regress_251_b CASCADE;
DROP OWNED BY regress_251_writer;
DROP ROLE regress_251_writer;

\echo 'regress_issue_251_schema_usage: PASS'

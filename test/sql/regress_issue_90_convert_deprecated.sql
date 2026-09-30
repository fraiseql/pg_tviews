-- Regression test (#90): pg_tviews_convert_existing_table() is deprecated.
--
-- The function could not succeed on PG18 (a Datum type error reading
-- information_schema) and, by design, replaced tv_x with a view over a literal
-- VALUES snapshot: no triggers, no refresh. Calling it must now fail with an
-- error that points at the supported conversion, pg_tviews_create() /
-- CREATE TABLE tv_x AS SELECT ..., and must leave the table untouched.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_90_convert_deprecated.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tv_legacy (pk_legacy int PRIMARY KEY, id uuid NOT NULL, data jsonb NOT NULL);
INSERT INTO tv_legacy VALUES (1, gen_random_uuid(), '{"a": 1}');

DO $$
DECLARE
    msg text;
BEGIN
    BEGIN
        PERFORM pg_tviews_convert_existing_table('tv_legacy');
        RAISE EXCEPTION 'pg_tviews_convert_existing_table succeeded; expected a deprecation error';
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT;
        IF msg NOT LIKE '%deprecated%' OR msg NOT LIKE '%pg_tviews_create%' THEN
            RAISE EXCEPTION 'error does not point at pg_tviews_create: %', msg;
        END IF;
    END;
END $$;

-- The table is left exactly as it was: still a plain table with its row.
DO $$
BEGIN
    IF (SELECT relkind FROM pg_class WHERE relname = 'tv_legacy') <> 'r'
       OR (SELECT count(*) FROM tv_legacy) <> 1 THEN
        RAISE EXCEPTION 'tv_legacy was modified by the deprecated function';
    END IF;
END $$;

\echo 'PASS regress_issue_90_convert_deprecated'

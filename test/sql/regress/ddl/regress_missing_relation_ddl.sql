-- A utility statement naming a relation that does not exist behaves as it does
-- without pg_tviews, for any role.
--
-- The hook looks for partitions a DROP, DETACH or TRUNCATE takes away, and asked
-- whether the caller owns (or may truncate) each name before checking that it
-- resolved: for a missing relation that check raised "cache lookup failed for
-- relation 0" (a role that is not a superuser) or "relation with OID 0 does not
-- exist" (TRUNCATE, any role), so DROP TABLE IF EXISTS failed in every migration.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_missing_relation_ddl.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
DROP ROLE IF EXISTS regress_missing_rel_user;
CREATE ROLE regress_missing_rel_user;
GRANT CREATE ON SCHEMA public TO regress_missing_rel_user;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'missing relation FAIL: %', what; END IF; END $$;

-- The error a statement raises, NULL if it succeeds.
CREATE FUNCTION raised(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE stmt;
    RETURN NULL;
EXCEPTION WHEN others THEN
    RETURN SQLSTATE || ' ' || SQLERRM;
END $$;

-- 1. IF EXISTS forms succeed, as an ordinary role and as a superuser.
SET ROLE regress_missing_rel_user;
CREATE TABLE mine (x int);
SELECT must(raised(s) IS NULL, s || ': ' || raised(s))
FROM unnest(ARRAY[
    'DROP TABLE IF EXISTS no_such_table',
    'DROP TABLE IF EXISTS no_such_schema.no_such_table',
    'DROP TABLE IF EXISTS mine, no_such_table',
    'DROP FOREIGN TABLE IF EXISTS no_such_table',
    'ALTER TABLE IF EXISTS no_such_table DETACH PARTITION no_such_partition']) s;
RESET ROLE;
SELECT must(raised('DROP TABLE IF EXISTS no_such_table') IS NULL, 'superuser DROP IF EXISTS');

-- 2. Without IF EXISTS, PostgreSQL's own error.
SET ROLE regress_missing_rel_user;
SELECT must(raised(s) = e, s || ': ' || coalesce(raised(s), 'no error'))
FROM (VALUES
    ('DROP TABLE no_such_table', '42P01 table "no_such_table" does not exist'),
    ('TRUNCATE no_such_table', '42P01 relation "no_such_table" does not exist'),
    ('TRUNCATE no_such_schema.t', '3F000 schema "no_such_schema" does not exist')) v(s, e);
RESET ROLE;
SELECT must(raised('TRUNCATE no_such_table') = '42P01 relation "no_such_table" does not exist',
            'superuser TRUNCATE: ' || coalesce(raised('TRUNCATE no_such_table'), 'no error'));

REVOKE CREATE ON SCHEMA public FROM regress_missing_rel_user;
DROP ROLE regress_missing_rel_user;

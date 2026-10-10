-- DDL run by a function inside another utility statement is intercepted (Q10).
--
-- The utility hook held its reentrancy guard for the whole statement it passed
-- on, so a DROP TABLE tv_* or a column RENAME issued by a function that EXECUTE
-- or CREATE TABLE AS runs was not seen: the TVIEW's catalog row and backing view
-- were left behind, or its definition kept the old column name. The guard now
-- covers only pg_tviews' own DDL.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_nested_ddl_intercepted.sql
-- expect-output: nested ddl: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user
$$);
SELECT pg_tviews_create('tv_badge', $$
    SELECT pk_user AS pk_badge, id, jsonb_build_object('n', name) AS data FROM tb_user
$$);

-- A column rename from a function run by EXECUTE follows into the definitions.
CREATE FUNCTION rename_name() RETURNS int LANGUAGE plpgsql AS $f$
BEGIN
    ALTER TABLE tb_user RENAME COLUMN name TO full_name;
    RETURN 1;
END $f$;
PREPARE do_rename AS SELECT rename_name();
EXECUTE do_rename;
DO $$
BEGIN
    IF (SELECT definition FROM tviews.pg_tview_meta WHERE entity = 'user') NOT LIKE '%full_name%' THEN
        RAISE EXCEPTION 'FAIL: a column rename run by EXECUTE was not followed: %',
            (SELECT definition FROM tviews.pg_tview_meta WHERE entity = 'user');
    END IF;
END $$;
UPDATE tb_user SET full_name = 'grace' WHERE pk_user = 1;
DO $$
BEGIN
    IF (SELECT data->>'name' FROM tv_user WHERE pk_user = 1) <> 'grace' THEN
        RAISE EXCEPTION 'FAIL: tv_user stale after the rename';
    END IF;
END $$;

-- A DROP TABLE tv_* from a function run by CREATE TABLE AS drops the TVIEW.
CREATE FUNCTION drop_badge() RETURNS int LANGUAGE plpgsql AS $f$
BEGIN
    DROP TABLE tv_badge;
    RETURN 1;
END $f$;
CREATE TABLE side_effect AS SELECT drop_badge() AS done;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'badge') THEN
        RAISE EXCEPTION 'FAIL: DROP TABLE tv_badge run by CREATE TABLE AS left its catalog row';
    END IF;
    IF to_regclass('tviews.public__tv_badge') IS NOT NULL THEN
        RAISE EXCEPTION 'FAIL: DROP TABLE tv_badge run by CREATE TABLE AS left its backing view';
    END IF;
END $$;

\echo 'nested ddl: PASS'

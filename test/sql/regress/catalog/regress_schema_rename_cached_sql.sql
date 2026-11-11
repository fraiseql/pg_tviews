-- SQL a backend cached for a TVIEW follows a rename of the schema it names: the
-- next write in the same backend reads the renamed tables, never a relation that
-- took the old name, and messages name the tables as they are called now.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_schema_rename_cached_sql.sql
-- expect-output: schema_rename_cached_sql: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'schema_rename_cached_sql FAIL: %', what; END IF; END $$;

DROP SCHEMA IF EXISTS s1 CASCADE;
DROP SCHEMA IF EXISTS s2 CASCADE;
CREATE SCHEMA s1;
CREATE TABLE s1.tb_author (pk_author int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE s1.tb_book (pk_book int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_author int REFERENCES s1.tb_author, title text);
INSERT INTO s1.tb_author (pk_author, name) SELECT g, 'a' || g FROM generate_series(1, 3) g;
INSERT INTO s1.tb_book (pk_book, fk_author, title) SELECT g, 1 + g % 3, 't' || g FROM generate_series(1, 3000) g;
SELECT tviews.pg_tviews_create('public.tv_book', $q$
    SELECT b.pk_book, b.id, jsonb_build_object('title', b.title, 'author', a.name) AS data
    FROM s1.tb_book b JOIN s1.tb_author a ON a.pk_author = b.fk_author $q$);
ANALYZE s1.tb_book, s1.tb_author;

-- 1. Warm every cache of this backend, rename the schema, write again.
UPDATE s1.tb_author SET name = 'b' WHERE pk_author = 1;
UPDATE s1.tb_book SET title = 'warm' WHERE pk_book = 3;
ALTER SCHEMA s1 RENAME TO s2;
UPDATE s2.tb_author SET name = 'c' WHERE pk_author = 1;
UPDATE s2.tb_book SET title = 'renamed' WHERE pk_book = 3;
SELECT must((SELECT count(*) FROM public.tv_book WHERE data->>'author' = 'c') = 1000,
            'a write after the schema rename did not reach tv_book');
SELECT must((SELECT data->>'title' FROM public.tv_book WHERE pk_book = 3) = 'renamed',
            'a book write after the schema rename did not reach tv_book');

-- 2. A relation that takes the old name is never read: the cached SQL names the
--    tables the TVIEW was created over, whatever they are called now.
CREATE SCHEMA s1;
CREATE TABLE s1.tb_book (pk_book int PRIMARY KEY, id uuid, fk_author int, title text);
CREATE TABLE s1.tb_author (pk_author int PRIMARY KEY, id uuid, name text);
UPDATE s2.tb_author SET name = 'd' WHERE pk_author = 1;
SELECT must((SELECT count(*) FROM public.tv_book WHERE data->>'author' = 'd') = 1000,
            'a write read the relations that took the old schema name');
DROP SCHEMA s1 CASCADE;

-- 3. The health check names the tables as they are called now.
SELECT must(message NOT LIKE '%s1.%', 'the health check names the old schema: ' || message)
FROM tviews.pg_tviews_check_health();
SELECT must(message LIKE '%writes to s2.tb_author scan%', 'seq_scans does not name s2.tb_author: ' || message)
FROM tviews.pg_tviews_check_health() WHERE component = 'seq_scans';

SELECT 'schema_rename_cached_sql: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP SCHEMA s2 CASCADE;

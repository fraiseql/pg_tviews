-- A child TVIEW whose parent key is nullable (LEFT JOIN through a plain view).
--
-- A child with a NULL fk_author is materialized with "author": null; setting the
-- key to a parent embeds it, setting it back to NULL drops it, and a bulk insert
-- mixing NULL and non-NULL keys lands every row. (NULL keys used to log a
-- spurious WARNING; a WARNING cannot be trapped from SQL, so the rows are what
-- is asserted.)
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/cascade/regress_nullable_fk.sql
--
-- expect-output: nullable fk: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'nullable fk FAIL: %', what; END IF; END $$;
CREATE FUNCTION both_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM assert_fresh('tv_author', 'pk_author', label);
    PERFORM assert_fresh('tv_article', 'pk_article', label);
END $$;

CREATE TABLE tb_author (
    pk_author INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    name TEXT NOT NULL
);
CREATE TABLE tb_article (
    pk_article INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    fk_author INTEGER REFERENCES tb_author(pk_author),
    title TEXT NOT NULL
);
INSERT INTO tb_author (name) VALUES ('Alice'), ('Bob');

CREATE VIEW author_prepared AS
SELECT pk_author, id, jsonb_build_object('id', id::text, 'name', name) AS data FROM tb_author;
CREATE VIEW article_prepared AS
SELECT a.pk_article, a.id, a.fk_author,
       jsonb_build_object('id', a.id::text, 'title', a.title, 'author', au.data) AS data
FROM tb_article a LEFT JOIN author_prepared au ON au.pk_author = a.fk_author;

SELECT pg_tviews_create('tv_author', 'SELECT pk_author, id, data FROM author_prepared');
SELECT pg_tviews_create('tv_article', 'SELECT pk_article, id, fk_author, data FROM article_prepared');
SELECT both_fresh('pg_tviews_create');

-- 1. NULL key.
INSERT INTO tb_article (title) VALUES ('Orphan Article');
SELECT both_fresh('an INSERT with a NULL FK');
SELECT must((SELECT count(*) FROM tv_article WHERE fk_author IS NULL) = 1,
            'orphan article missing from tv_article');
SELECT must((SELECT data->'author' FROM tv_article WHERE data->>'title' = 'Orphan Article') = 'null'::jsonb,
            'orphan article author is not JSON null');

-- 2. Valid key.
INSERT INTO tb_article (fk_author, title) VALUES (1, 'Alice Article');
SELECT both_fresh('an INSERT with a valid FK');
SELECT must((SELECT data->'author'->>'name' FROM tv_article WHERE fk_author = 1) = 'Alice',
            'Alice''s article does not embed Alice');

-- 3. NULL to valid.
UPDATE tb_article SET fk_author = 2 WHERE fk_author IS NULL;
SELECT both_fresh('an UPDATE of a NULL FK to a valid one');
SELECT must((SELECT data->'author'->>'name' FROM tv_article WHERE data->>'title' = 'Orphan Article') = 'Bob',
            'formerly orphan article does not embed Bob');

-- 4. Valid to NULL.
UPDATE tb_article SET fk_author = NULL WHERE title = 'Orphan Article';
SELECT both_fresh('an UPDATE of a valid FK to NULL');
SELECT must((SELECT fk_author IS NULL AND data->'author' = 'null'::jsonb
             FROM tv_article WHERE data->>'title' = 'Orphan Article'),
            'article set back to NULL FK still embeds an author');

-- 5. Bulk insert mixing NULL and non-NULL keys.
INSERT INTO tb_article (fk_author, title) VALUES
    (NULL, 'Bulk Orphan 1'), (1, 'Bulk Alice 1'), (NULL, 'Bulk Orphan 2'),
    (2, 'Bulk Bob 1'), (NULL, 'Bulk Orphan 3');
SELECT both_fresh('a mixed NULL/non-NULL bulk INSERT');
SELECT must((SELECT (count(*) FILTER (WHERE fk_author IS NULL), count(*) FILTER (WHERE fk_author IS NOT NULL))
             FROM tv_article) = (4::bigint, 3::bigint),
            'expected 4 NULL-FK and 3 valid-FK articles');
SELECT must((SELECT data->'author'->>'name' FROM tv_article WHERE data->>'title' = 'Bulk Bob 1') = 'Bob',
            'bulk-inserted Bob article does not embed Bob');

\echo 'nullable fk: PASS'

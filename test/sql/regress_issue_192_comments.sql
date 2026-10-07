-- Regression test for issue #192: comments in a TVIEW definition, an apostrophe
-- in them included, are accepted as PostgreSQL accepts them; a comment marker
-- inside a string or a dollar-quoted body is not a comment.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_192_comments.sql
--
-- expect-output: issue #192 comments: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#192 FAIL: %', what; END IF; END $$;

CREATE TABLE tb_a (pk_a bigint PRIMARY KEY, id uuid DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_a (pk_a, name) VALUES (1, 'a');

-- 1. The issue: an apostrophe in a -- comment.
SELECT tviews.pg_tviews_create('tv_a', $q$
SELECT pk_a, id,
    -- the entity's name
    name
FROM tb_a $q$);
UPDATE tb_a SET name = 'a2';
SELECT assert_fresh('tv_a', 'pk_a', 'an apostrophe in a -- comment');

-- 2. create_or_replace: a block comment with an apostrophe, a comment after FROM,
--    and comment markers inside strings, which stay strings.
SELECT tviews.pg_tviews_create_or_replace('tv_a', $q$
SELECT pk_a, id, /* it's the name */ name,
    'not -- a comment' AS dashes, 'nor /* this */' AS stars, E'it\'s' AS escaped,
    $x$ -- still a string's body $x$ AS dollar
FROM tb_a -- the table's rows
WHERE name <> 'it''s -- quoted' $q$);
SELECT must((SELECT dashes = 'not -- a comment' AND stars = 'nor /* this */' AND escaped = 'it''s'
                    AND dollar = ' -- still a string''s body '
             FROM tv_a WHERE pk_a = 1), 'strings with comment markers were changed');
UPDATE tb_a SET name = 'a3';
SELECT assert_fresh('tv_a', 'pk_a', 'after create_or_replace with comments');

-- 3. Nested block comments, and a comment in the key's line.
SELECT tviews.pg_tviews_drop('tv_a');
SELECT tviews.pg_tviews_create('tv_a', $q$
SELECT /* outer /* inner's */ still outer's */ pk_a, -- the key's column
       id, name FROM tb_a $q$);
SELECT must((SELECT count(*) FROM tv_a) = 1, 'nested comments');

SELECT 'issue #192 comments: PASS' AS result;

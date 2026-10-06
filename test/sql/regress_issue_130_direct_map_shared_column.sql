-- Regression test (#130): a column that also feeds an expression is not patched
-- directly.
--
-- The direct-patch map (#56) kept `bio -> bio` for
-- jsonb_build_object('bio', bio, 'ub', upper(bio)), so UPDATE ... SET bio patched
-- only "bio" and left "ub" stale. Such a column now recomputes the row, while a
-- column used only bare keeps the fast path.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_130_direct_map_shared_column.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (
    pk_user int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL,
    bio     text NOT NULL,
    city    text NOT NULL
);
INSERT INTO tb_user VALUES (1, default, 'alice', 'a', 'paris');

SELECT pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id,
           jsonb_build_object(
               'name', u.name,
               'bio', u.bio,
               'bio_upper', upper(u.bio),
               'city', u.city,
               'where', jsonb_build_object('city', u.city)) AS data
    FROM tb_user u $$);

CREATE FUNCTION assert_fresh(step text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF (SELECT data FROM tv_user WHERE pk_user = 1)
       IS DISTINCT FROM (SELECT data FROM tviews.public__tv_user WHERE pk_user = 1) THEN
        RAISE EXCEPTION '#130 FAIL after %: tv_user % <> tviews.public__tv_user %', step,
            (SELECT data FROM tv_user WHERE pk_user = 1),
            (SELECT data FROM tviews.public__tv_user WHERE pk_user = 1);
    END IF;
END $$;

CREATE FUNCTION applied() RETURNS bigint LANGUAGE sql AS
    $$ SELECT (pg_tviews_queue_stats()->>'direct_patches_applied')::bigint $$;

UPDATE tb_user SET bio = 'b' WHERE pk_user = 1;
SELECT assert_fresh('bio update');
UPDATE tb_user SET city = 'lyon' WHERE pk_user = 1;
SELECT assert_fresh('city update');

-- A bare-only column still takes the fast path.
DO $$
DECLARE before bigint := applied();
BEGIN
    UPDATE tb_user SET name = 'alice2' WHERE pk_user = 1;
    IF applied() <> before + 1 THEN
        RAISE EXCEPTION '#130 FAIL: name update did not use the direct patch';
    END IF;
END $$;
SELECT assert_fresh('name update');

-- Only "name" is mapped: bio feeds bio_upper, city feeds the nested object.
DO $$ BEGIN
    IF (SELECT direct_map_columns FROM pg_tview_meta WHERE entity = 'user') <> '{name}' THEN
        RAISE EXCEPTION '#130 FAIL: direct map is %',
            (SELECT direct_map_columns FROM pg_tview_meta WHERE entity = 'user');
    END IF;
END $$;

SELECT 'issue #130 direct map shared column: PASS' AS result;
-- expect-output: issue #130 direct map shared column: PASS

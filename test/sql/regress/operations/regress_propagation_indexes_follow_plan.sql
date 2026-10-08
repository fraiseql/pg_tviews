-- The propagation indexes are the plan's embed lookups, whatever their names.
--
-- A parent TVIEW's rows are found by the columns holding an embedded TVIEW's
-- key (the plan's lookups). pg_tviews_ensure_propagation_indexes() creates the
-- index each one needs, and pg_tviews_profile() reports the missing ones; a
-- column is not one because it is called fk_*.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_propagation_indexes_follow_plan.sql
-- expect-output: propagation indexes follow the plan: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint REFERENCES tb_user, fk_tag bigint, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
INSERT INTO tb_post (pk_post, fk_user, fk_tag, title) VALUES (1, 1, 7, 'notes');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user
$$);
-- author_pk holds tv_user's key; fk_tag holds no TVIEW's key.
SELECT pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user AS author_pk, p.fk_tag,
           jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user
$$);

-- Every index leading with a column that is not the key goes (an older release
-- created none).
DO $$
DECLARE i text;
BEGIN
    FOR i IN SELECT quote_ident(ic.relname) FROM pg_index x
             JOIN pg_class ic ON ic.oid = x.indexrelid
             JOIN pg_attribute a ON a.attrelid = x.indrelid AND a.attnum = x.indkey[0]
             WHERE x.indrelid = 'tv_post'::regclass AND a.attname IN ('author_pk', 'fk_tag')
    LOOP
        EXECUTE 'DROP INDEX ' || i;
    END LOOP;
END $$;

DO $$
DECLARE
    reported text[] := ARRAY(SELECT tviews.pg_tviews_ensure_propagation_indexes('post', true));
    missing text[] := (SELECT missing_propagation_indexes FROM tviews.pg_tviews_profile('post'));
BEGIN
    IF array_length(reported, 1) IS DISTINCT FROM 1 OR reported[1] NOT LIKE '%("author_pk", "pk_post")%' THEN
        RAISE EXCEPTION 'FAIL: ensure_propagation_indexes reports %, not the author_pk index', reported;
    END IF;
    IF missing IS DISTINCT FROM ARRAY['author_pk'] THEN
        RAISE EXCEPTION 'FAIL: pg_tviews_profile reports % as missing, not {author_pk}', missing;
    END IF;
END $$;

SELECT tviews.pg_tviews_ensure_propagation_indexes('post');
DO $$
BEGIN
    IF EXISTS (SELECT tviews.pg_tviews_ensure_propagation_indexes('post', true)) THEN
        RAISE EXCEPTION 'FAIL: the index is still reported missing after it was created';
    END IF;
    IF (SELECT cardinality(missing_propagation_indexes) FROM tviews.pg_tviews_profile('post')) <> 0 THEN
        RAISE EXCEPTION 'FAIL: pg_tviews_profile still reports a missing index';
    END IF;
END $$;

\echo 'propagation indexes follow the plan: PASS'

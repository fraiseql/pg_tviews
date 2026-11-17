-- A parent reading a child TVIEW's row whole is recomputed, not patched, when
-- the child is patched: the patch copies the child's document into the parent's,
-- and the parent reads more of the child row than that document.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_wholerow_nested_parent.sql
--
-- expect-output: whole-row nested parent: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

-- The refreshes forced because the definition reads a table whole, by reason
-- (test-hooks builds; NULL in others).
CREATE FUNCTION stopgap(reason text) RETURNS bigint LANGUAGE sql AS $$
    SELECT CASE WHEN tviews.pg_tviews_show_queue() ? 'patch_stats'
                THEN coalesce((tviews.pg_tviews_show_queue()->'patch_stats'->>('stopgap:' || reason))::bigint, 0)
           END $$;
CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'u1'), (2, 'u2');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) AS data FROM tb_user u $$) IS NOT NULL AS created;
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', u.data, 'raw', md5(u::text)) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$) IS NOT NULL AS created;

UPDATE tb_user SET name = 'u1b' WHERE pk_user = 1;
SELECT assert_fresh('tv_user', 'pk_user', 'the patched child');
SELECT assert_fresh('tv_post', 'pk_post', 'a parent reading the patched child row whole');

BEGIN;
UPDATE tb_user SET name = 'u2b' WHERE pk_user = 2;
COMMIT;
SELECT assert_fresh('tv_post', 'pk_post', 'the same, in an explicit transaction');

SELECT must(stopgap('wholerow') IS NULL OR stopgap('wholerow') > 0, 'no stopgap:wholerow counted');

SELECT 'whole-row nested parent: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;

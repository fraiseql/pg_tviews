-- A mutation function that writes and then reads the TVIEW back, in the same
-- call, reads its own write: directly, through a helper function, and through a
-- function called by the outer statement (no tv_* in that statement's own
-- range table).
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_mutation_reads_own_write.sql
--
-- expect-output: mutation reads its own write: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', upper(p.title), 'author', jsonb_build_object('name', a.name)) AS data
    FROM tb_post p JOIN tb_user a ON a.pk_user = p.fk_user $$) IS NOT NULL AS created;

-- The document as the view yields it now, rendered in the frame.
CREATE FUNCTION viewed(k int) RETURNS jsonb LANGUAGE sql STABLE AS $$
    SELECT data FROM tviews.public__tv_post WHERE pk_post = k $$;
CREATE FUNCTION stored(k int) RETURNS jsonb LANGUAGE sql STABLE AS $$
    SELECT data FROM tv_post WHERE pk_post = k $$;

-- Reads tv_post in its own statement.
CREATE FUNCTION rename_direct(k int, t text) RETURNS jsonb LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_post SET title = t WHERE pk_post = k;
    RETURN (SELECT data FROM tv_post WHERE pk_post = k);
END $$;
-- Reads it through a helper whose statement is the only one naming tv_post.
CREATE FUNCTION rename_helper(k int, t text) RETURNS jsonb LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_user SET name = t WHERE pk_user = (SELECT fk_user FROM tb_post WHERE pk_post = k);
    RETURN stored(k);
END $$;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;

-- What each mutation returned, compared in a later statement: a STABLE read of
-- the view in the same statement would see the snapshot from before the write.
CREATE TEMP TABLE got (step int, what text, doc jsonb);
INSERT INTO got SELECT 1, 'a mutation reading tv_post in its own statement', rename_direct(1, 'p2');
SELECT must(doc = viewed(1), what) FROM got WHERE step = 1;
INSERT INTO got SELECT 2, 'a mutation reading tv_post through a helper', rename_helper(1, 'bea');
SELECT must(doc = viewed(1), what) FROM got WHERE step = 2;
-- The outer statement names no tv_*: the read is inside the called function.
INSERT INTO got SELECT 3, 'a mutation called from a statement naming no tv_*', (SELECT rename_helper(1, 'cal'));
SELECT must(doc = viewed(1), what) FROM got WHERE step = 3;
SELECT assert_fresh('tv_post', 'pk_post', 'the mutations');

SELECT 'mutation reads its own write: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;

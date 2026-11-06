-- pg_tviews.max_queue_size bounds the refresh keys a transaction queues on
-- every path: a write mapped to many keys at once, and a resync, which refuses
-- more keys than the bound before it reads any row.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_resync_queue_bound.sql
-- expect-output: queue bound: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'queue bound: %', what; END IF; END $$;
-- The SQLSTATE of `sql`, or NULL when it succeeds.
CREATE FUNCTION state_of(sql text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE s text;
BEGIN
    EXECUTE sql;
    RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS s = RETURNED_SQLSTATE;
    RETURN s;
END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann'), (2, 'bob');
INSERT INTO tb_post (pk_post, fk_user, title)
SELECT g, CASE WHEN g <= 1001 THEN 1 ELSE 2 END, 'p' || g FROM generate_series(1, 2001) g;
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', upper(u.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);

SET pg_tviews.max_queue_size = 1000;

-- A write reaching 1,001 TVIEW rows at once; then one reaching 1,000. The
-- author is computed from the joined row: its rows are recomputed by key.
SELECT must(state_of($$UPDATE tb_user SET name = 'ann2' WHERE pk_user = 1$$) = '54000',
            'a write mapped to more keys than the bound');
SELECT must(state_of($$UPDATE tb_user SET name = 'bob2' WHERE pk_user = 2$$) IS NULL,
            'a write mapped to as many keys as the bound');
SELECT must(count(*) = 1000, 'the rows of the write within the bound')
FROM tv_post WHERE data->>'author' = 'BOB2';

-- A resync of 1,001 keys; then one of 1,000, given twice.
SELECT must(state_of($$SELECT tviews.pg_tviews_resync('tb_post',
                (SELECT array_agg(g) FROM generate_series(1, 1001) g))$$) = '54000',
            'a resync of more keys than the bound');
SELECT must(state_of($$SELECT tviews.pg_tviews_resync('tb_post',
                (SELECT array_agg(g) FROM generate_series(1, 1000) g)
                || (SELECT array_agg(g) FROM generate_series(1, 1000) g))$$) IS NULL,
            'a resync of as many distinct keys as the bound');
RESET pg_tviews.max_queue_size;

SELECT 'queue bound: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_post, tb_user;
DROP FUNCTION must(boolean, text), state_of(text);

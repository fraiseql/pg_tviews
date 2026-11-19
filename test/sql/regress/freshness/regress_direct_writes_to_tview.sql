-- pg_tviews is the only writer of a tv_* table: an INSERT, UPDATE or DELETE
-- from outside a refresh, its owner's included, fails with 55000 (it would leave
-- the table and the documents embedding it differing from their views until the
-- next refresh). The owner's TRUNCATE, followed by pg_tviews_refresh(), is the
-- one write allowed: after it, the TVIEW and every TVIEW embedding it are fresh.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/freshness/regress_direct_writes_to_tview.sql
--
-- known-failing: #321 (D14)
-- expect-output: direct writes to a TVIEW: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

-- Raise unless `stmt` fails with SQLSTATE `want`.
CREATE FUNCTION refused(stmt text, want text, what text) RETURNS void LANGUAGE plpgsql AS $f$
DECLARE state text;
BEGIN
    BEGIN
        EXECUTE stmt;
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE;
        IF state <> want THEN
            RAISE EXCEPTION 'FAIL: % failed with % (want %): %', what, state, want, SQLERRM;
        END IF;
        RETURN;
    END;
    RAISE EXCEPTION 'FAIL: % was accepted', what;
END $f$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ann'), (2, 'ben');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1'), (2, 2, 'p2');
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) AS data FROM tb_user u $$) IS NOT NULL AS created;
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$) IS NOT NULL AS created;

SELECT refused($s$ UPDATE tv_user SET data = '{"name": "eve"}' WHERE pk_user = 1 $s$, '55000',
               'an UPDATE of tv_user by its owner');
SELECT refused($s$ DELETE FROM tv_user WHERE pk_user = 2 $s$, '55000', 'a DELETE from tv_user by its owner');
SELECT refused($s$ INSERT INTO tv_user (pk_user, id, data) VALUES (3, gen_random_uuid(), '{}') $s$, '55000',
               'an INSERT into tv_user by its owner');
SELECT assert_fresh('tv_user', 'pk_user', 'the refused writes');
SELECT assert_fresh('tv_post', 'pk_post', 'the refused writes');

TRUNCATE tv_user;
SELECT tviews.pg_tviews_refresh('tv_user');
SELECT assert_fresh('tv_user', 'pk_user', 'TRUNCATE and pg_tviews_refresh');
SELECT assert_fresh('tv_post', 'pk_post', 'TRUNCATE of the TVIEW it embeds and its refresh');

SELECT 'direct writes to a TVIEW: PASS' AS result;
DROP EXTENSION pg_tviews CASCADE;

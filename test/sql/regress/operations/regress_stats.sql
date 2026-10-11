-- Per-TVIEW refresh statistics readable from any session (#220, ADR 0221):
--   1. a session reads what another session's transactions refreshed, per TVIEW;
--   2. two TVIEWs' counters are not mixed;
--   3. an aborted transaction's work counts;
--   4. a reset zeroes one TVIEW or all; a dropped TVIEW leaves the view;
--   5. any role reads the view; the reset is an operator's.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_stats.sql
-- expect-output: stats: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'stats FAIL: %', what; END IF; END $$;
CREATE FUNCTION outcome(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'ok';
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ' ' || SQLERRM; END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text);
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);
SELECT tviews.pg_tviews_stats_reset();
SELECT must((SELECT count(*) FROM tviews.stats) = 2, 'one row per TVIEW');
SELECT must(NOT bool_or(untracked) AND bool_and(view_recomputes = 0 AND rows_written = 0),
            'after a reset: ' || string_agg(entity || '=' || rows_written, ','))
FROM tviews.stats;

-- 1 and 2. Another session's writes, read from a new session.
INSERT INTO tb_user VALUES (1, DEFAULT, 'ann'), (2, DEFAULT, 'bob');
INSERT INTO tb_post VALUES (1, DEFAULT, 1, 'p1'), (2, DEFAULT, 1, 'p2'), (3, DEFAULT, 2, 'p3');
UPDATE tb_user SET name = 'Ann' WHERE pk_user = 1;
DELETE FROM tb_post WHERE pk_post = 3;
\c
SET client_min_messages TO WARNING;
SELECT must(rows_written >= 3 AND rows_deleted = 0 AND refresh_ms > 0 AND stats_reset IS NOT NULL
            AND NOT untracked,
            'tv_user: ' || row(view_recomputes, rows_written, rows_deleted, refresh_ms)::text)
FROM tviews.stats WHERE entity = 'user';
SELECT must(rows_written >= 5 AND rows_deleted = 1 AND view_recomputes >= 3,
            'tv_post: ' || row(view_recomputes, rows_written, rows_deleted)::text)
FROM tviews.stats WHERE entity = 'post';
SELECT must(schema = 'public' AND name = 'tv_post', 'names: ' || schema || '.' || name)
FROM tviews.stats WHERE entity = 'post';

-- 3. An aborted transaction's work counts.
CREATE TABLE stats_before AS SELECT rows_written FROM tviews.stats WHERE entity = 'user';
BEGIN;
UPDATE tb_user SET name = 'Bob' WHERE pk_user = 2;
ROLLBACK;
\c
SET client_min_messages TO WARNING;
SELECT must((SELECT rows_written FROM tviews.stats WHERE entity = 'user') >
            (SELECT rows_written FROM stats_before), 'the rolled-back refresh was not counted');
DROP TABLE stats_before;

-- 4. Reset one TVIEW, then all; a dropped TVIEW leaves the view.
SELECT tviews.pg_tviews_stats_reset('post');
SELECT must((SELECT rows_written FROM tviews.stats WHERE entity = 'post') = 0
            AND (SELECT rows_written FROM tviews.stats WHERE entity = 'user') > 0,
            'reset of one TVIEW');
SELECT tviews.pg_tviews_stats_reset();
SELECT must((SELECT sum(rows_written) FROM tviews.stats) = 0, 'reset of every TVIEW');
SELECT tviews.pg_tviews_drop('post');
SELECT must((SELECT array_agg(entity) FROM tviews.stats) = '{user}', 'a dropped TVIEW is still listed');

-- 5. Any role reads the view; the reset is an operator's.
CREATE ROLE regress_stats_reader;
SET ROLE regress_stats_reader;
SELECT must((SELECT count(*) FROM tviews.stats) = 1, 'a plain role cannot read tviews.stats');
SELECT must(outcome('SELECT tviews.pg_tviews_stats_reset()') LIKE '42501 %',
            'a plain role may reset the statistics');
RESET ROLE;
DROP ROLE regress_stats_reader;

\echo 'stats: PASS'

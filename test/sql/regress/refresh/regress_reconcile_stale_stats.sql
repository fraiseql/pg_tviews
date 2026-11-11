-- A full refresh's cost does not depend on the TVIEW table's statistics. A TVIEW
-- emptied and analyzed, then refilled, has statistics saying it holds no row; the
-- statements that bring it to its view join the whole table with the whole view,
-- and a plan that trusts that estimate evaluates the view once per row (an
-- aggregate here: seconds at a few thousand rows, quadratic beyond). The refresh
-- finishes well within the timeout, the rows equal the view, and a row whose id
-- moves to another key in one refresh keeps the unique index on id satisfied.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_reconcile_stale_stats.sql
-- expect-output: reconcile_stale_stats: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'reconcile_stale_stats FAIL: %', what; END IF; END $$;

-- tb_flag is read in uncorrelated subqueries: a write to it refreshes the whole TVIEW.
CREATE TABLE tb_flag (pk_flag int PRIMARY KEY, min_total int NOT NULL, label text NOT NULL);
INSERT INTO tb_flag VALUES (1, 0, 'a');
CREATE TABLE tb_grp (pk_grp int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_line (pk_line int PRIMARY KEY, fk_grp int NOT NULL REFERENCES tb_grp, n int NOT NULL);
INSERT INTO tb_grp (pk_grp, name) SELECT g, 'g' || g FROM generate_series(1, 8000) g;
INSERT INTO tb_line (pk_line, fk_grp, n) SELECT g, 1 + g % 8000, g % 7 FROM generate_series(1, 40000) g;
CREATE INDEX ON tb_line (fk_grp);
SELECT tviews.pg_tviews_create('tv_grp', $q$
    SELECT g.pk_grp, g.id,
           jsonb_build_object('name', g.name, 'total', s.total,
                              'label', (SELECT f.label FROM tb_flag f WHERE f.pk_flag = 1)) AS data
    FROM tb_grp g
    JOIN (SELECT l.fk_grp, sum(l.n) AS total FROM tb_line l GROUP BY l.fk_grp) s ON s.fk_grp = g.pk_grp
    WHERE s.total >= (SELECT f.min_total FROM tb_flag f WHERE f.pk_flag = 1) $q$,
    options => '{"uncascaded_policy": "full_refresh"}');
ALTER TABLE tv_grp SET (autovacuum_enabled = off);
VACUUM ANALYZE tb_flag, tb_grp, tb_line;

-- Emptied by a refresh, analyzed empty, refilled: the statistics say 0 rows.
UPDATE tb_flag SET min_total = 1000000;
ANALYZE tv_grp;
UPDATE tb_flag SET min_total = 0;
SELECT must(reltuples = 0, 'the statistics are not stale: ' || reltuples)
FROM pg_class WHERE oid = 'tv_grp'::regclass;
SELECT must((SELECT count(*) FROM tv_grp) = 8000, 'the refill did not fill tv_grp');

-- 1. Each of the three statements runs over the whole table: every row updated,
--    half deleted, half inserted again.
SET statement_timeout = '5s';
UPDATE tb_flag SET label = 'b';
UPDATE tb_flag SET min_total = 15;
UPDATE tb_flag SET min_total = 0;
RESET statement_timeout;
SELECT must((SELECT count(*) FROM tv_grp t JOIN tviews.public__tv_grp v USING (pk_grp)
             WHERE t.data = v.data AND t.id = v.id) = 8000
            AND (SELECT count(*) FROM tv_grp) = 8000, 'tv_grp differs from its view');

-- 2. In one refresh, a row leaves and another key takes its id: the row leaving
--    goes first, so the unique index on id holds.
SELECT id AS moved_id FROM tb_grp WHERE pk_grp = 1 \gset
BEGIN;
DELETE FROM tb_line WHERE fk_grp = 1;
DELETE FROM tb_grp WHERE pk_grp = 1;
INSERT INTO tb_grp (pk_grp, id, name) VALUES (9001, :'moved_id', 'moved');
INSERT INTO tb_line (pk_line, fk_grp, n) VALUES (40001, 9001, 3);
UPDATE tb_flag SET label = 'c';
COMMIT;
SELECT must((SELECT pk_grp FROM tv_grp WHERE id = :'moved_id') = 9001, 'the id did not move to 9001');
SELECT must(NOT EXISTS (SELECT FROM tv_grp WHERE pk_grp = 1), 'pk_grp 1 is still in tv_grp');

SELECT 'reconcile_stale_stats: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE tb_line, tb_grp, tb_flag;

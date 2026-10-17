-- Regression test: a TVIEW restored without its rows is refilled.
--
-- pg_dump decides about the extension's configuration (tviews.pg_tview_valid)
-- separately from the application's tables: a dump that leaves out a TVIEW's rows
-- by name (--exclude-table-data) restores it empty. An UNLOGGED TVIEW keeps its
-- validity row, and a LOGGED one has no validity check at all: both came back
-- empty and trusted, and stayed empty. Now the first write reaching a TVIEW that
-- was empty refills it when its view computes rows the write did not produce. A
-- TVIEW that is empty because its view computes nothing is written as usual: its
-- first row is reported, not refilled.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lifecycle/regress_restore_excluded_data.sql
-- expect-output: restore_excluded_data: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

-- pg_dump/pg_restore must match the server's major version.
SELECT current_setting('server_version_num')::int / 10000 AS server_major \gset
\setenv PGTV_SERVER_MAJOR :server_major
\! test "$(pg_dump --version | sed -E 's/^[^0-9]*([0-9]+).*/\1/')" = "$PGTV_SERVER_MAJOR"
\if :SHELL_ERROR
  \echo 'SKIP: pg_dump on PATH is not the server''s major version'
  \quit
\endif
\set dumpdir `mktemp -d`
\setenv PGTV_DUMPDIR :dumpdir
\setenv PGTV_SRC :DBNAME

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'restore_excluded_data FAIL: %', what; END IF; END $$;

-- An UNLOGGED TVIEW and a LOGGED one, three rows each; and an empty one.
CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text NOT NULL);
CREATE TABLE tb_team (pk_team bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text NOT NULL);
CREATE TABLE tb_note (pk_note bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      body text NOT NULL);
INSERT INTO tb_user (pk_user, name) SELECT g, 'u' || g FROM generate_series(1, 3) g;
INSERT INTO tb_team (pk_team, name) SELECT g, 't' || g FROM generate_series(1, 3) g;
SELECT tviews.pg_tviews_create('tv_user',
    $$SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user$$,
    options => '{"logged": false}');
SELECT tviews.pg_tviews_create('tv_team',
    $$SELECT pk_team, id, jsonb_build_object('name', name) AS data FROM tb_team$$);
-- Embeds tv_team's document: restored empty with it.
CREATE TABLE tb_seat (pk_seat bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_team bigint NOT NULL REFERENCES tb_team, label text NOT NULL);
INSERT INTO tb_seat (pk_seat, fk_team, label) SELECT g, g, 's' || g FROM generate_series(1, 3) g;
SELECT tviews.pg_tviews_create('tv_seat',
    $$SELECT s.pk_seat, s.id, s.fk_team, jsonb_build_object('label', s.label, 'team', t.data) AS data
      FROM tb_seat s JOIN tv_team t ON t.pk_team = s.fk_team$$);
SELECT tviews.pg_tviews_create('tv_note',
    $$SELECT pk_note, id, jsonb_build_object('body', body) AS data FROM tb_note$$);

-- The TVIEWs' rows left out by name; the extension's configuration kept.
\! pg_dump -Fc --exclude-table-data='public.tv_*' -f "$PGTV_DUMPDIR/excluded.dump" "$PGTV_SRC"
\if :SHELL_ERROR
  DO $$ BEGIN RAISE EXCEPTION 'restore_excluded_data FAIL: pg_dump failed'; END $$;
\endif
\! dropdb --if-exists "${PGTV_SRC}_excl" && createdb "${PGTV_SRC}_excl" && pg_restore --exit-on-error -d "${PGTV_SRC}_excl" "$PGTV_DUMPDIR/excluded.dump"
\if :SHELL_ERROR
  DO $$ BEGIN RAISE EXCEPTION 'restore_excluded_data FAIL: pg_restore failed'; END $$;
\endif
\set restored `echo "${PGTV_SRC}_excl"`

-- ========================================================================
-- 1. The first write refills each TVIEW restored empty.
-- ========================================================================
\c :restored
SET client_min_messages TO WARNING;
SELECT must((SELECT count(*) FROM tv_user) = 0 AND (SELECT count(*) FROM tv_team) = 0,
            'the dump carried the TVIEWs'' rows');
-- A write rolled back first: it leaves pages in tv_user, and the next one still refills.
BEGIN;
UPDATE tb_user SET name = 'rolled back' WHERE pk_user = 2;
ROLLBACK;
\c
SET client_min_messages TO WARNING;
-- tv_seat first: tv_team, which it reads and which no write reached, is refilled before it.
UPDATE tb_seat SET label = 's1!' WHERE pk_seat = 1;
SELECT must((SELECT count(*) FROM tv_seat) = 3 AND (SELECT count(*) FROM tv_team) = 3,
            'tv_seat refilled without tv_team: seats ' || (SELECT count(*) FROM tv_seat)
            || ', teams ' || (SELECT count(*) FROM tv_team));
SELECT must((SELECT count(*) FROM tv_seat WHERE data->'team'->>'name' IS NULL) = 0,
            'tv_seat embeds an empty tv_team');
\c
SET client_min_messages TO WARNING;
SELECT must(EXISTS (SELECT 1 FROM tviews.pg_tview_valid WHERE entity = 'user'),
            'the dump did not carry tv_user''s validity row');
UPDATE tb_user SET name = 'u1!' WHERE pk_user = 1;
SELECT must((SELECT count(*) FROM tv_user) = 3,
            'UNLOGGED tv_user restored empty was not refilled: '
            || (SELECT count(*) FROM tv_user) || ' rows');
SELECT must((SELECT data->>'name' FROM tv_user WHERE pk_user = 1) = 'u1!', 'tv_user missed the write');
\c
SET client_min_messages TO WARNING;
UPDATE tb_team SET name = 't1!' WHERE pk_team = 1;
SELECT must((SELECT count(*) FROM tv_team) = 3,
            'LOGGED tv_team restored empty was not refilled: '
            || (SELECT count(*) FROM tv_team) || ' rows');
SELECT must((SELECT data->'team'->>'name' FROM tv_seat WHERE pk_seat = 1) = 't1!',
            'tv_seat missed the team''s write');
SELECT must((SELECT data->>'name' FROM tv_team WHERE pk_team = 1) = 't1!', 'tv_team missed the write');

-- ========================================================================
-- 2. An empty TVIEW whose view computes nothing: its first row is written and
--    reported, not refilled.
-- ========================================================================
\c
SET client_min_messages TO WARNING;
SELECT full_refreshes AS full_before FROM tviews.stats WHERE entity = 'note' \gset
BEGIN;
INSERT INTO tb_note (pk_note, body) VALUES (1, 'n1');
SELECT tviews.pg_tviews_flush_and_report(include_data => false) AS report \gset
COMMIT;
SELECT must(full_refreshes = :full_before, 'the first row of an empty TVIEW refilled it')
FROM tviews.stats WHERE entity = 'note';
SELECT must(:'report'::jsonb->'invalidated_types' = '[]'
            AND jsonb_array_length(:'report'::jsonb->'updated') = 1,
            'the first row of an empty TVIEW is not reported as a row: ' || :'report');

-- ========================================================================
-- Cleanup
-- ========================================================================
\c postgres
\! dropdb --if-exists "${PGTV_SRC}_excl"; rm -rf "$PGTV_DUMPDIR"

\echo 'restore_excluded_data: PASS'

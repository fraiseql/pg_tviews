-- Regression test (#236): a logical restore that carries an UNLOGGED TVIEW's rows
-- also carries their validity.
--
-- pg_tviews trusts an UNLOGGED TVIEW's rows while tviews.pg_tview_valid holds a
-- row for it; PostgreSQL empties both together on a crash restart or a
-- promotion, and the next write (or pg_tviews_rebuild_all(only_empty)) refills
-- the TVIEW. The validity rows were keyed by table oid and not dumped, so every
-- restored UNLOGGED TVIEW counted as reset and was filled again, rows and all.
-- They are keyed by entity and dumped as extension configuration: pg_dump
-- writes them exactly when it writes UNLOGGED tables' rows, so a restore brings
-- back both or neither.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lifecycle/regress_restore_validity.sql
-- expect-output: restore_validity: PASS

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
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#236 FAIL: %', what; END IF; END $$;

-- An UNLOGGED TVIEW and a LOGGED one, three rows each.
CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text NOT NULL);
CREATE TABLE tb_team (pk_team bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text NOT NULL);
INSERT INTO tb_user (pk_user, name) SELECT g, 'u' || g FROM generate_series(1, 3) g;
INSERT INTO tb_team (pk_team, name) SELECT g, 't' || g FROM generate_series(1, 3) g;
SELECT tviews.pg_tviews_create('tv_user',
    $$SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user$$,
    options => '{"logged": false}');
SELECT tviews.pg_tviews_create('tv_team',
    $$SELECT pk_team, id, jsonb_build_object('name', name) AS data FROM tb_team$$);

-- The source's dumps: everything, and without UNLOGGED tables' rows.
\! pg_dump -Fc -f "$PGTV_DUMPDIR/all.dump" "$PGTV_SRC" && pg_dump -Fc --no-unlogged-table-data -f "$PGTV_DUMPDIR/nounlogged.dump" "$PGTV_SRC"
\if :SHELL_ERROR
  DO $$ BEGIN RAISE EXCEPTION '#236 FAIL: pg_dump failed'; END $$;
\endif

-- ========================================================================
-- 1. A plain dump: the UNLOGGED TVIEW comes back with its rows, trusted.
-- ========================================================================
\! dropdb --if-exists "${PGTV_SRC}_all" && createdb "${PGTV_SRC}_all" && pg_restore --exit-on-error -d "${PGTV_SRC}_all" "$PGTV_DUMPDIR/all.dump"
\if :SHELL_ERROR
  DO $$ BEGIN RAISE EXCEPTION '#236 FAIL: restoring the plain dump failed'; END $$;
\endif
\set restored :DBNAME '_all'
\c :restored
SET client_min_messages TO WARNING;

SELECT must((SELECT count(*) FROM tv_user) = 3, 'the plain restore lost tv_user''s rows');
SELECT coalesce(string_agg(r::text, ', '), '') AS refilled
FROM tviews.pg_tviews_rebuild_all(only_empty => true) r \gset
SELECT must(:'refilled' = '', 'only_empty refilled a TVIEW restored with its rows: ' || :'refilled');
-- The first write refreshes its row and fills nothing.
SELECT full_refreshes AS full_before FROM tviews.stats WHERE entity = 'user' \gset
UPDATE tb_user SET name = 'u1!' WHERE pk_user = 1;
SELECT must(full_refreshes = :full_before, 'the first write after the restore refilled tv_user')
FROM tviews.stats WHERE entity = 'user';
SELECT must((SELECT data->>'name' FROM tv_user WHERE pk_user = 1) = 'u1!', 'tv_user missed the write');

-- A rebuild of every TVIEW reports the rows it wrote: all of them.
SELECT must(array_agg(r.entity || ':' || r.rows_inserted ORDER BY r.entity) = '{team:3,user:3}',
            'rebuild_all(only_empty => false): '
            || array_agg(r.entity || ':' || r.rows_inserted ORDER BY r.entity)::text)
FROM tviews.pg_tviews_rebuild_all(only_empty => false) r;

-- Validity is keyed by entity, so it survives a rename of the table. A reset
-- (its row deleted, the table emptied, and a new backend: a reset ends them all)
-- is refilled under the new name.
ALTER TABLE tv_user RENAME TO tv_user_archive;
TRUNCATE tv_user_archive;
DELETE FROM tviews.pg_tview_valid;
\c
SET client_min_messages TO WARNING;
UPDATE tb_user SET name = 'u2!' WHERE pk_user = 2;
SELECT must((SELECT count(*) FROM tv_user_archive) = 3, 'the renamed TVIEW was not refilled');
SELECT must(EXISTS (SELECT 1 FROM tviews.pg_tview_valid WHERE entity = 'user'),
            'the refilled TVIEW has no validity row');

-- ========================================================================
-- 2. Without UNLOGGED tables' rows: the TVIEW is empty and refilled once.
-- ========================================================================
\c postgres
\! dropdb --if-exists "${PGTV_SRC}_nounlogged" && createdb "${PGTV_SRC}_nounlogged" && pg_restore --exit-on-error -d "${PGTV_SRC}_nounlogged" "$PGTV_DUMPDIR/nounlogged.dump"
\if :SHELL_ERROR
  DO $$ BEGIN RAISE EXCEPTION '#236 FAIL: restoring the dump without UNLOGGED rows failed'; END $$;
\endif
\set restored `echo "${PGTV_SRC}_nounlogged"`
\c :restored
SET client_min_messages TO WARNING;

SELECT must((SELECT count(*) FROM tv_user) = 0 AND (SELECT count(*) FROM tv_team) = 3,
            'the dump without UNLOGGED rows restored tv_user''s rows or lost tv_team''s');
SELECT must(array_agg(r.entity || ':' || r.rows_inserted) = '{user:3}',
            'only_empty after a restore without UNLOGGED rows: '
            || coalesce(array_agg(r.entity || ':' || r.rows_inserted)::text, 'nothing'))
FROM tviews.pg_tviews_rebuild_all(only_empty => true) r;
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tviews_rebuild_all(only_empty => true)),
            'a second only_empty refilled again');

-- ========================================================================
-- Cleanup
-- ========================================================================
\c postgres
\! dropdb --if-exists "${PGTV_SRC}_all"; dropdb --if-exists "${PGTV_SRC}_nounlogged"; rm -rf "$PGTV_DUMPDIR"

\echo 'restore_validity: PASS'

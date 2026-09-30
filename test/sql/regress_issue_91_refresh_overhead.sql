-- Regression test for issue #91: fixed per-refresh overhead.
--
-- A warm single-row refresh made 7-10 catalog lookups before its one real statement,
-- and the per-row recompute path evaluated the backing view twice. TVIEW metadata is
-- now cached per backend, and the recompute upserts straight from the view.
--
-- Caches are only safe if other backends' changes reach them: the last cycles change
-- TVIEW metadata from another backend (psql \!) while this session's caches are warm.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_91_refresh_overhead.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_item (
    pk_item BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    n       INT
);
INSERT INTO tb_item (n) SELECT g FROM generate_series(1, 100) g;
CREATE TABLE tv_item AS SELECT pk_item, id, jsonb_build_object('n', n) AS data FROM tb_item;

CREATE FUNCTION lookups() RETURNS INT LANGUAGE sql AS
$$ SELECT (pg_tviews_queue_stats()->>'catalog_lookups')::int $$;
CREATE FUNCTION pk_scans() RETURNS BIGINT LANGUAGE sql AS $$
    SELECT pg_stat_clear_snapshot();
    SELECT idx_scan FROM pg_stat_user_indexes WHERE indexrelname = 'tb_item_pkey'
$$;
CREATE FUNCTION must(ok BOOLEAN, msg TEXT) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT ok THEN RAISE EXCEPTION '#91 FAIL: %', msg; END IF; END $$;
CREATE FUNCTION in_sync() RETURNS BOOLEAN LANGUAGE sql AS $$
    SELECT count(*) = 0 FROM v_item v FULL JOIN tv_item t USING (pk_item)
    WHERE t.data IS DISTINCT FROM v.data
$$;

-- Warm both paths.
UPDATE tb_item SET n = n + 1 WHERE pk_item = 1;
SET pg_tviews.direct_patch_enabled = off;
UPDATE tb_item SET n = n + 1 WHERE pk_item = 1;

-- ========================================================================
-- Cycle 1: a warm recompute refresh evaluates the view once, with no lookups
-- ========================================================================
SELECT pg_stat_force_next_flush();
SELECT pk_scans() AS scans0, lookups() AS l0 \gset
UPDATE tb_item SET n = n + 1 WHERE pk_item = 2;
SELECT lookups() AS l1 \gset
SELECT pg_stat_force_next_flush();
-- The base UPDATE itself is one scan of tb_item_pkey, the view evaluation the other.
SELECT must(pk_scans() - :scans0 = 2,
            format('recompute scanned tb_item_pkey %s times, expected 2', pk_scans() - :scans0));
SELECT must(:l1 - :l0 = 0, format('warm recompute made %s catalog lookups', :l1 - :l0));

-- ========================================================================
-- Cycle 2: a warm direct-patch refresh makes no lookups
-- ========================================================================
SET pg_tviews.direct_patch_enabled = on;
SELECT lookups() AS l2 \gset
UPDATE tb_item SET n = n + 1 WHERE pk_item = 3;
SELECT must(lookups() - :l2 = 0, format('warm direct patch made %s catalog lookups', lookups() - :l2));

-- Deletes and inserts still go through the single-evaluation path.
DELETE FROM tb_item WHERE pk_item = 4;
INSERT INTO tb_item (n) VALUES (1000);
SELECT must(in_sync(), 'tv_item out of sync after update/delete/insert');

-- ========================================================================
-- Cycle 3: another backend renames the column the TVIEW reads
-- ========================================================================
\setenv PGTV_DB :DBNAME
\! psql -X -q -v ON_ERROR_STOP=1 -d "$PGTV_DB" -c 'ALTER TABLE tb_item RENAME COLUMN n TO num'
\if :SHELL_ERROR
  DO $$ BEGIN RAISE EXCEPTION '#91 FAIL: rename in the other backend failed'; END $$;
\endif
UPDATE tb_item SET num = num + 1 WHERE pk_item = 5;
SET pg_tviews.direct_patch_enabled = off;
UPDATE tb_item SET num = num + 1 WHERE pk_item = 6;
SET pg_tviews.direct_patch_enabled = on;
SELECT must(in_sync(), 'tv_item out of sync after a rename in another backend');

-- ========================================================================
-- Cycle 4: another backend drops and recreates the TVIEW
-- ========================================================================
\! psql -X -q -v ON_ERROR_STOP=1 -d "$PGTV_DB" -c "SELECT pg_tviews_drop('item', true, true)" -c "CREATE TABLE tv_item AS SELECT pk_item, id, jsonb_build_object('n', num) AS data FROM tb_item"
\if :SHELL_ERROR
  DO $$ BEGIN RAISE EXCEPTION '#91 FAIL: drop/create in the other backend failed'; END $$;
\endif
UPDATE tb_item SET num = num + 1 WHERE pk_item = 7;
SELECT must(in_sync(), 'tv_item out of sync after it was recreated in another backend');

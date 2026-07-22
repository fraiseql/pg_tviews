-- Regression test for issue #56 (Phase 5): differential byte-identity + fallbacks.
--
-- Acceptance criterion: an eligible fast-path UPDATE yields tv_*.data byte-identical
-- to a full recompute. Each eligible scenario runs the mutation, snapshots the data,
-- forces a recompute (pg_tviews_refresh), and asserts equality — plus 0 view
-- recomputes on the fast path. Each fallback scenario asserts a recompute ran and
-- the result is correct.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_56_differential.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION _rc() RETURNS bigint LANGUAGE sql AS
  $$ SELECT (pg_tviews_queue_stats()->>'view_recomputes')::bigint $$;

-- ══ Part A: eligible scenarios — byte-identity + zero recomputes ═════════════
DROP TABLE IF EXISTS tv_thing CASCADE; DROP VIEW IF EXISTS v_thing CASCADE;
DROP TABLE IF EXISTS tb_thing CASCADE;
CREATE TABLE tb_thing (
    pk_thing INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    s TEXT, n_int INTEGER, n_big BIGINT, n_small SMALLINT,
    flag BOOLEAN, uu UUID, jj JSONB, headline TEXT
);
INSERT INTO tb_thing (s, n_int, n_big, n_small, flag, uu, jj, headline)
VALUES ('s0', 1, 100, 5, true, '11111111-1111-1111-1111-111111111111', '{"k":"v"}', 'H0');

-- Key names deliberately differ from column names for several fields.
SELECT pg_tviews_create('tv_thing', $TVIEW$
    SELECT pk_thing, id, jsonb_build_object(
        's', s, 'i', n_int, 'big', n_big, 'small', n_small,
        'flag', flag, 'uid', uu, 'meta', jj, 'headline_key', headline
    ) AS data
    FROM tb_thing
$TVIEW$);

-- Differential helper: run `mutation`, snapshot fast data, force recompute,
-- assert byte-identity AND that the fast path did zero recomputes.
CREATE FUNCTION _diff(mutation text, label text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE fast jsonb; rec jsonb; rc0 bigint;
BEGIN
  rc0 := _rc();
  EXECUTE mutation;
  IF _rc() <> rc0 THEN
    RAISE EXCEPTION '#56 diff [%]: eligible mutation did % view recompute(s)', label, _rc() - rc0;
  END IF;
  SELECT data INTO fast FROM tv_thing WHERE pk_thing = 1;
  PERFORM pg_tviews_refresh('thing');
  SELECT data INTO rec FROM tv_thing WHERE pk_thing = 1;
  IF fast IS DISTINCT FROM rec THEN
    RAISE EXCEPTION '#56 diff [%]: fast % <> recompute %', label, fast, rec;
  END IF;
END $$;

SELECT _diff($$UPDATE tb_thing SET s = 'text-changed' WHERE pk_thing = 1$$, 'text');
SELECT _diff($$UPDATE tb_thing SET n_int = 42 WHERE pk_thing = 1$$, 'int4');
SELECT _diff($$UPDATE tb_thing SET n_big = 9000000000 WHERE pk_thing = 1$$, 'int8');
SELECT _diff($$UPDATE tb_thing SET n_small = 7 WHERE pk_thing = 1$$, 'int2');
SELECT _diff($$UPDATE tb_thing SET flag = false WHERE pk_thing = 1$$, 'bool');
SELECT _diff($$UPDATE tb_thing SET uu = '22222222-2222-2222-2222-222222222222' WHERE pk_thing = 1$$, 'uuid');
SELECT _diff($$UPDATE tb_thing SET jj = '{"k2":[1,2,3]}'::jsonb WHERE pk_thing = 1$$, 'jsonb');
SELECT _diff($$UPDATE tb_thing SET headline = 'H1' WHERE pk_thing = 1$$, 'key<>column');
SELECT _diff($$UPDATE tb_thing SET s = 'multi', n_int = 99 WHERE pk_thing = 1$$, 'multi-column');
-- NULL value: set-to-null, byte-identical to jsonb_build_object emitting json null.
SELECT _diff($$UPDATE tb_thing SET s = NULL WHERE pk_thing = 1$$, 'null-value');
DO $$ BEGIN
  IF (SELECT data ? 's' AND data->>'s' IS NULL FROM tv_thing WHERE pk_thing=1) IS NOT TRUE THEN
    RAISE EXCEPTION '#56 diff [null-value]: key not present-as-null';
  END IF;
END $$;
-- No-op update (unchanged value): datumIsEqual finds no changed column, so nothing
-- is captured and the key plain-enqueues (recompute) — current behaviour, kept.
-- Only correctness is asserted (a no-op patch would be an optimisation, not a fix).
DO $$
DECLARE before jsonb;
BEGIN
  SELECT data INTO before FROM tv_thing WHERE pk_thing = 1;
  UPDATE tb_thing SET n_int = n_int WHERE pk_thing = 1;
  IF (SELECT data FROM tv_thing WHERE pk_thing = 1) IS DISTINCT FROM before THEN
    RAISE EXCEPTION '#56 diff [no-op]: value-preserving update changed the data';
  END IF;
END $$;

-- ══ Part B: fallback scenarios — recompute runs, result correct ══════════════

-- (#15) Unsupported mapped type (timestamptz): capture declines ⇒ recompute.
DROP TABLE IF EXISTS tv_ts CASCADE; DROP VIEW IF EXISTS v_ts CASCADE;
DROP TABLE IF EXISTS tb_ts CASCADE;
CREATE TABLE tb_ts (pk_ts INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE, at TIMESTAMPTZ);
INSERT INTO tb_ts (at) VALUES ('2020-01-01T00:00:00Z');
SELECT pg_tviews_create('tv_ts', $TVIEW$
    SELECT pk_ts, id, jsonb_build_object('at', at) AS data FROM tb_ts
$TVIEW$);
DO $$
DECLARE rc0 bigint;
BEGIN
  rc0 := _rc();
  UPDATE tb_ts SET at = '2021-06-15T12:30:00Z' WHERE pk_ts = 1;
  IF _rc() = rc0 THEN
    RAISE EXCEPTION '#56 diff [timestamptz]: unsupported type did not fall back to recompute';
  END IF;
  IF (SELECT (data->>'at')::timestamptz FROM tv_ts WHERE pk_ts = 1) <> '2021-06-15T12:30:00Z'::timestamptz THEN
    RAISE EXCEPTION '#56 diff [timestamptz]: recompute wrong';
  END IF;
END $$;

-- (#11) Column also projected as a separate tv output column ⇒ recompute keeps both
-- the tv column and the data key in sync.
DROP TABLE IF EXISTS tv_proj CASCADE; DROP VIEW IF EXISTS v_proj CASCADE;
DROP TABLE IF EXISTS tb_proj CASCADE;
CREATE TABLE tb_proj (pk_proj INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE, title TEXT);
INSERT INTO tb_proj (title) VALUES ('orig');
SELECT pg_tviews_create('tv_proj', $TVIEW$
    SELECT pk_proj, id, title, jsonb_build_object('title', title) AS data FROM tb_proj
$TVIEW$);
DO $$
DECLARE rc0 bigint;
BEGIN
  rc0 := _rc();
  UPDATE tb_proj SET title = 'changed' WHERE pk_proj = 1;
  IF _rc() = rc0 THEN
    RAISE EXCEPTION '#56 diff [projected-col]: expected recompute (title is a tv column)';
  END IF;
  IF (SELECT title FROM tv_proj WHERE pk_proj = 1) <> 'changed'
     OR (SELECT data->>'title' FROM tv_proj WHERE pk_proj = 1) <> 'changed' THEN
    RAISE EXCEPTION '#56 diff [projected-col]: tv column and data key out of sync';
  END IF;
END $$;

-- (#12) Filter-column change: row leaves the tview, then returns (DELETE-on-missing).
DROP TABLE IF EXISTS tv_flt CASCADE; DROP VIEW IF EXISTS v_flt CASCADE;
DROP TABLE IF EXISTS tb_flt CASCADE;
CREATE TABLE tb_flt (pk_flt INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                     id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE, status TEXT, body TEXT);
INSERT INTO tb_flt (status, body) VALUES ('active', 'b1');
SELECT pg_tviews_create('tv_flt', $TVIEW$
    SELECT pk_flt, id, jsonb_build_object('body', body) AS data
    FROM tb_flt WHERE status = 'active'
$TVIEW$);
DO $$ BEGIN
  IF (SELECT count(*) FROM tv_flt WHERE pk_flt = 1) <> 1 THEN
    RAISE EXCEPTION '#56 diff [filter]: setup — row should be present';
  END IF;
  -- Leave the filter: row must disappear from the tview.
  UPDATE tb_flt SET status = 'archived' WHERE pk_flt = 1;
  IF (SELECT count(*) FROM tv_flt WHERE pk_flt = 1) <> 0 THEN
    RAISE EXCEPTION '#56 diff [filter]: row did not leave the tview on filter change';
  END IF;
  -- Return to the filter: row must reappear.
  UPDATE tb_flt SET status = 'active' WHERE pk_flt = 1;
  IF (SELECT data->>'body' FROM tv_flt WHERE pk_flt = 1) <> 'b1' THEN
    RAISE EXCEPTION '#56 diff [filter]: row did not return on filter change';
  END IF;
END $$;

-- (#17) DISTINCT ON tview: different refresh machinery ⇒ never fast-pathed.
DROP TABLE IF EXISTS tv_dedup CASCADE; DROP VIEW IF EXISTS v_dedup CASCADE;
DROP TABLE IF EXISTS tb_dedup CASCADE;
CREATE TABLE tb_dedup (pk_dedup INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                       id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
                       grp INTEGER, ord INTEGER, label TEXT);
INSERT INTO tb_dedup (grp, ord, label) VALUES (1, 1, 'a'), (1, 2, 'b');
SELECT pg_tviews_create('tv_dedup', $TVIEW$
    SELECT DISTINCT ON (grp) grp AS pk_dedup, id,
           jsonb_build_object('label', label) AS data
    FROM tb_dedup ORDER BY grp, ord DESC
$TVIEW$);
-- DISTINCT ON refresh uses refresh_by_dedup_key (not the recompute_view_row path),
-- so the signal is that it is never fast-patched: direct_patches_applied is flat.
DO $$
DECLARE ap0 bigint;
BEGIN
  ap0 := (pg_tviews_queue_stats()->>'direct_patches_applied')::bigint;
  UPDATE tb_dedup SET label = 'b2' WHERE pk_dedup = 2;
  IF (pg_tviews_queue_stats()->>'direct_patches_applied')::bigint <> ap0 THEN
    RAISE EXCEPTION '#56 diff [distinct-on]: a DISTINCT ON tview was fast-patched';
  END IF;
  IF (SELECT data->>'label' FROM tv_dedup WHERE pk_dedup = 1) <> 'b2' THEN
    RAISE EXCEPTION '#56 diff [distinct-on]: dedup refresh wrong';
  END IF;
END $$;

SELECT 'issue #56 differential: PASS' AS result;

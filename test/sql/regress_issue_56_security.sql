-- Regression test for issue #56 (Phase 7 security): exotic JSONB keys are omitted
-- from the direct-patch column map, so nothing user-controlled is ever
-- interpolated into generated SQL. Patch values are always bound as JSONB
-- parameters; only path segments (from the analyzer's \w+ capture) reach an
-- ARRAY['…'] literal, and even those are single-quote escaped.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_56_security.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP TABLE IF EXISTS tv_thing CASCADE; DROP VIEW IF EXISTS v_thing CASCADE;
DROP TABLE IF EXISTS tb_thing CASCADE;
CREATE TABLE tb_thing (pk_thing INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                       id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
                       safe TEXT, danger TEXT);
INSERT INTO tb_thing (safe, danger) VALUES ('s0', 'd0');

-- A key containing a quote + SQL-ish payload must NOT enter the column map.
SELECT pg_tviews_create('tv_thing', $TVIEW$
    SELECT pk_thing, id, jsonb_build_object(
        'safe', safe,
        'weird'')-- ; DROP TABLE tb_thing; --', danger
    ) AS data
    FROM tb_thing
$TVIEW$);

-- The safe key is mapped; the exotic key is omitted (so `danger` is never fast-pathed).
DO $$
DECLARE cols text[]; keys text[];
BEGIN
  SELECT direct_map_columns, direct_map_keys INTO cols, keys
  FROM pg_tview_meta WHERE entity = 'thing';
  IF NOT (cols @> ARRAY['safe']::text[]) THEN
    RAISE EXCEPTION '#56 security FAIL: safe key not mapped (cols=%)', cols;
  END IF;
  IF cols @> ARRAY['danger']::text[] THEN
    RAISE EXCEPTION '#56 security FAIL: exotic-key column entered the map (cols=%)', cols;
  END IF;
  IF EXISTS (SELECT 1 FROM unnest(keys) k WHERE k LIKE '%DROP%' OR k LIKE '%''%') THEN
    RAISE EXCEPTION '#56 security FAIL: exotic key survived in the map (keys=%)', keys;
  END IF;
END $$;

-- The tview still materialises and updates correctly (the exotic key via recompute,
-- the safe key via the fast path). tb_thing must still exist (no injection executed).
UPDATE tb_thing SET safe = 's1' WHERE pk_thing = 1;      -- eligible fast path
UPDATE tb_thing SET danger = 'd1' WHERE pk_thing = 1;    -- unmapped ⇒ recompute
DO $$ BEGIN
  IF (SELECT to_regclass('tb_thing')) IS NULL THEN
    RAISE EXCEPTION '#56 security FAIL: base table was dropped — injection executed!';
  END IF;
  IF (SELECT data->>'safe' FROM tv_thing WHERE pk_thing = 1) <> 's1' THEN
    RAISE EXCEPTION '#56 security FAIL: safe fast-path value wrong';
  END IF;
  IF (SELECT data->>'weird'')-- ; DROP TABLE tb_thing; --' FROM tv_thing WHERE pk_thing = 1) <> 'd1' THEN
    RAISE EXCEPTION '#56 security FAIL: exotic-key value not refreshed by recompute';
  END IF;
END $$;

SELECT 'issue #56 security: PASS' AS result;

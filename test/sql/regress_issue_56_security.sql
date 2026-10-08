-- Regression test for issue #56 (security): a JSONB key holding a quote and an SQL
-- payload is a key like any other. The column map is stored and patches are
-- applied through bound parameters only, so the key is mapped, patched under its
-- own name, and nothing it contains is ever executed.
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

-- Both keys are mapped, the exotic one under its exact name.
DO $$
DECLARE cols text[]; keys text[];
BEGIN
  SELECT direct_map_columns, direct_map_keys INTO cols, keys
  FROM pg_tview_meta WHERE entity = 'thing';
  IF NOT (cols @> ARRAY['safe', 'danger']::text[]) THEN
    RAISE EXCEPTION '#56 security FAIL: a key is not mapped (cols=%)', cols;
  END IF;
  IF NOT (keys @> ARRAY['weird'')-- ; DROP TABLE tb_thing; --']::text[]) THEN
    RAISE EXCEPTION '#56 security FAIL: the exotic key is not mapped as written (keys=%)', keys;
  END IF;
END $$;

-- Both are patched; tb_thing must still exist (no injection executed).
UPDATE tb_thing SET safe = 's1' WHERE pk_thing = 1;
UPDATE tb_thing SET danger = 'd1' WHERE pk_thing = 1;
DO $$ BEGIN
  IF (SELECT to_regclass('tb_thing')) IS NULL THEN
    RAISE EXCEPTION '#56 security FAIL: base table was dropped — injection executed!';
  END IF;
  IF (SELECT data->>'safe' FROM tv_thing WHERE pk_thing = 1) <> 's1' THEN
    RAISE EXCEPTION '#56 security FAIL: safe fast-path value wrong';
  END IF;
  IF (SELECT data->>'weird'')-- ; DROP TABLE tb_thing; --' FROM tv_thing WHERE pk_thing = 1) <> 'd1' THEN
    RAISE EXCEPTION '#56 security FAIL: exotic-key value not patched';
  END IF;
END $$;

SELECT 'issue #56 security: PASS' AS result;

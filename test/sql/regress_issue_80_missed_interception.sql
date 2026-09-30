-- Regression test (#80, decision U-D2): when the ProcessUtility hook did not see a
-- CREATE TABLE tv_* AS (pg_tviews not preloaded, or any future interception gap), the
-- event trigger must fail the statement with an actionable ERROR instead of leaving a
-- silent plain table that deploy tools cannot detect.
--
-- shared_preload_libraries is cluster-wide, so the miss is simulated with the test-only
-- GUC pg_tviews.test_skip_ctas_intercept, which makes the hook skip the statement.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_80_missed_interception.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
    title TEXT
);
INSERT INTO tb_post (title) VALUES ('one');

SET pg_tviews.test_skip_ctas_intercept = on;

DO $$
DECLARE
  msg text; hint text;
BEGIN
  BEGIN
    EXECUTE $q$CREATE TABLE tv_post AS
        SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post$q$;
    RAISE EXCEPTION 'FAIL: CTAS not intercepted but no error was raised';
  EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT, hint = PG_EXCEPTION_HINT;
    IF msg LIKE 'FAIL:%' THEN RAISE; END IF;
    IF msg NOT LIKE '%tv_post%' OR hint NOT LIKE '%shared_preload_libraries%' THEN
      RAISE EXCEPTION 'FAIL: error not actionable: % / hint: %', msg, hint;
    END IF;
  END;
END $$;

-- The failed statement rolled back: no plain table left behind.
DO $$ BEGIN
  IF to_regclass('tv_post') IS NOT NULL THEN
    RAISE EXCEPTION 'FAIL: a plain tv_post was left behind';
  END IF;
END $$;

-- With interception working again the same CTAS converts normally.
RESET pg_tviews.test_skip_ctas_intercept;
CREATE TABLE tv_post AS
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_tview_meta WHERE entity = 'post') THEN
    RAISE EXCEPTION 'FAIL: tv_post not registered after interception restored';
  END IF;
END $$;

\echo 'regress_issue_80_missed_interception: OK'

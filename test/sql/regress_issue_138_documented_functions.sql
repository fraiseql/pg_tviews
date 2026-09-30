-- Regression test (#138): every function the published docs name exists.
--
-- The docs described 36 pg_tviews_* functions that no release ever shipped
-- (pg_tviews_refresh_one, pg_tviews_install_stmt_triggers, ...), so a reader or a
-- tool generating SQL from them got "function does not exist". This test extracts
-- every pg_tviews_*( and pg_tview_*( name from README.md, INTEGRATION_GUIDE.md and
-- docs/ (history excluded: docs/archive, docs/adr) and checks it against the
-- functions of a fresh CREATE EXTENSION. It runs from inside the repository.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_138_documented_functions.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\set documented `cd "$(git rev-parse --show-toplevel)" && git ls-files README.md INTEGRATION_GUIDE.md 'docs/*.md' | grep -v -e '^docs/archive/' -e '^docs/adr/' | xargs grep -ohE '\bpg_tviews?_[a-z0-9_]+[(]' | tr -d '(' | sort -u | paste -sd, -`

CREATE TEMP TABLE documented AS
    SELECT unnest(string_to_array(:'documented', ',')) AS name;

DO $$
DECLARE
    missing TEXT;
BEGIN
    IF (SELECT count(*) FROM documented) < 20 THEN
        RAISE EXCEPTION '#138 FAIL: found only % documented names; is this run from the repository?',
            (SELECT count(*) FROM documented);
    END IF;
    SELECT string_agg(d.name, ', ' ORDER BY d.name) INTO missing
    FROM documented d
    WHERE NOT EXISTS (SELECT 1 FROM pg_proc p
                      WHERE p.proname = d.name
                        AND p.pronamespace = 'tviews'::regnamespace)
      -- Defined by scripts/auto-convert/auto_convert_tviews.sql, documented with it.
      AND d.name NOT IN ('pg_tviews_auto_convert', 'pg_tviews_auto_convert_plan');
    IF missing IS NOT NULL THEN
        RAISE EXCEPTION '#138 FAIL: the docs name functions pg_tviews does not have: %', missing;
    END IF;
END $$;

DROP EXTENSION pg_tviews CASCADE;

SELECT 'issue #138 documented functions: PASS' AS result;
-- expect-output: issue #138 documented functions: PASS

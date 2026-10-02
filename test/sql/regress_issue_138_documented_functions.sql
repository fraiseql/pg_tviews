-- Regression test (#138): every pg_tviews object the published docs name exists.
--
-- The docs described 33 pg_tviews_* functions that no release ever shipped
-- (pg_tviews_refresh_one, pg_tviews_install_stmt_triggers, ...), so a reader or a
-- tool generating SQL from them got "function does not exist". This test extracts
-- every pg_tviews_* and pg_tview_* name from README.md, INTEGRATION_GUIDE.md and
-- docs/ (Markdown and JSON; history excluded: docs/archive, docs/adr) and checks it
-- against the functions, relations, types, triggers and event triggers of a fresh
-- CREATE EXTENSION. It runs from inside the repository.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_issue_138_documented_functions.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

\set documented `cd "$(git rev-parse --show-toplevel)" && git ls-files README.md INTEGRATION_GUIDE.md 'docs/*.md' 'docs/*.json' | grep -v -e '^docs/archive/' -e '^docs/adr/' | xargs grep -ohE '\bpg_tviews?_[a-z0-9_]+' | sort -u | paste -sd, -`

CREATE TEMP TABLE documented AS
    SELECT unnest(string_to_array(:'documented', ',')) AS name;

CREATE TEMP TABLE known AS
    SELECT proname::text AS name FROM pg_proc WHERE pronamespace = 'tviews'::regnamespace
    UNION SELECT relname FROM pg_class WHERE relnamespace = 'tviews'::regnamespace
    UNION SELECT typname FROM pg_type WHERE typnamespace = 'tviews'::regnamespace
    UNION SELECT evtname FROM pg_event_trigger
    UNION SELECT tgname FROM pg_trigger
          WHERE tgrelid IN (SELECT oid FROM pg_class
                            WHERE relnamespace = 'tviews'::regnamespace)
    -- Names the docs give to things users create: databases, roles, backups, and
    -- the functions scripts/auto-convert/auto_convert_tviews.sql defines.
    UNION SELECT unnest(ARRAY[
        'pg_tviews_test', 'pg_tviews_benchmark', 'pg_tviews_recovery_test',
        'pg_tviews_user', 'pg_tviews_admin', 'pg_tviews_test_user',
        'pg_tview_meta_backup', 'pg_tviews_auto_convert', 'pg_tviews_auto_convert_plan']);

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
    WHERE d.name NOT IN (SELECT name FROM known);
    IF missing IS NOT NULL THEN
        RAISE EXCEPTION '#138 FAIL: the docs name pg_tviews objects that do not exist: %', missing;
    END IF;
END $$;

DROP EXTENSION pg_tviews CASCADE;

SELECT 'issue #138 documented objects: PASS' AS result;
-- expect-output: issue #138 documented objects: PASS

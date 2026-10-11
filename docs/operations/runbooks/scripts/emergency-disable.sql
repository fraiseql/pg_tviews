-- pg_tviews emergency: defer refreshing TVIEWs to the end of a transaction, and
-- bring them up to date afterwards.
-- Run: psql -X -v ON_ERROR_STOP=1 -d <database> -f docs/operations/runbooks/scripts/emergency-disable.sql
--
-- pg_tviews_suspend_triggers() lasts until the end of the transaction: it lets one
-- transaction write base tables without refreshing TVIEWs row by row (a bulk load),
-- records which TVIEWs it skipped, and refreshes them once on resume or at COMMIT. Other sessions and other transactions keep refreshing.
-- This script shows the state and the commands; it changes nothing.

\echo '=== pg_tviews emergency controls ==='

\echo ''
\echo '1. Current state (this transaction)'
SELECT tviews.pg_tviews_is_suspended() AS suspended,
       tviews.pg_tviews_suspended_entities() AS changed_while_suspended;

\echo ''
\echo '2. Health problems that may call for it'
SELECT component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info'
ORDER BY component;

\echo ''
\echo '3. Commands (run them in the transaction that writes):'
\echo '   BEGIN;'
\echo '   SELECT tviews.pg_tviews_suspend_triggers();   -- stop refreshing in this transaction'
\echo '   ... writes ...'
\echo '   SELECT tviews.pg_tviews_resume_triggers();    -- resume; skipped TVIEWs are refreshed'
\echo '   COMMIT;'
\echo ''
\echo '   Bring one TVIEW, or all of them, back to their views:'
\echo '   SELECT tviews.pg_tviews_refresh(''<entity>'');'
\echo '   SELECT tviews.pg_tviews_refresh_all();'

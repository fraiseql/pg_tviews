-- pg_tviews refresh status: how each TVIEW is kept up to date, and how recently
-- its rows changed.
-- Run: psql -X -v ON_ERROR_STOP=1 -d <database> -f docs/operations/runbooks/scripts/refresh-status.sql
--
-- Read-only. pg_tviews refreshes TVIEW rows inside the transaction that writes the
-- base tables; there is no refresh job and no queue table to inspect.

\echo '=== pg_tviews refresh status ==='

\echo ''
\echo '1. How writes to each base table reach each TVIEW'
\echo '   local: key read off the row; mapped: one query per statement;'
\echo '   propagated: through an embedded TVIEW; all_keys: see uncascaded_policy'
SELECT r.schema, r.name, k.key AS base_table, k.value AS kind, r.uncascaded_policy
FROM tviews.registry r, pg_catalog.jsonb_each_text(r.cascade_kinds) k
ORDER BY r.schema, r.name, k.key;

\echo ''
\echo '2. TVIEWs to re-register after an upgrade (SELECT * FROM tviews.pg_tviews_reregister_all())'
SELECT schema, name FROM tviews.registry WHERE needs_reregister ORDER BY schema, name;

\echo ''
\echo '3. Last content change and write activity per TVIEW table'
SELECT pg_catalog.format(
           'SELECT %L AS tview, max(t.updated_at) AS last_change, '
           '(SELECT n_tup_ins FROM pg_catalog.pg_stat_user_tables WHERE relid = %L::regclass) AS rows_inserted, '
           '(SELECT n_tup_upd FROM pg_catalog.pg_stat_user_tables WHERE relid = %L::regclass) AS rows_updated, '
           '(SELECT n_tup_del FROM pg_catalog.pg_stat_user_tables WHERE relid = %L::regclass) AS rows_deleted '
           'FROM %I.%I t',
           r.schema || '.' || r.name,
           pg_catalog.quote_ident(r.schema) || '.' || pg_catalog.quote_ident(r.name),
           pg_catalog.quote_ident(r.schema) || '.' || pg_catalog.quote_ident(r.name),
           pg_catalog.quote_ident(r.schema) || '.' || pg_catalog.quote_ident(r.name),
           r.schema, r.name)
FROM tviews.registry r
WHERE r.schema IS NOT NULL
ORDER BY r.schema, r.name
\gexec

\echo ''
\echo '4. Is refresh suspended in this transaction?'
SELECT tviews.pg_tviews_is_suspended() AS suspended,
       tviews.pg_tviews_suspended_entities() AS changed_while_suspended;

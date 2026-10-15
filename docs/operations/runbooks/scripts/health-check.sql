-- pg_tviews health check
-- Run: psql -X -v ON_ERROR_STOP=1 -d <database> -f docs/operations/runbooks/scripts/health-check.sql
--
-- Read-only. Every relation and function it uses ships with pg_tviews.

\echo '=== pg_tviews health check ==='

\echo ''
\echo '1. Versions: library, installed extension, catalog revision, read contract'
SELECT tviews.pg_tviews_version() AS library,
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews') AS extension,
       tviews.pg_tviews_catalog_revision() AS catalog_revision,
       tviews.contract_version() AS contract_version;

\echo ''
\echo '2. Health check (status ok / warning / error per component)'
SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check()
ORDER BY CASE severity WHEN 'error' THEN 1 WHEN 'warning' THEN 2 ELSE 3 END, component;

\echo ''
\echo '3. Registered TVIEWs'
SELECT schema, name, options->'logged' AS logged, needs_reregister,
       uncascaded_tables, options->>'uncascaded_policy' AS uncascaded_policy, cascade_kinds
FROM tviews.registry
ORDER BY schema, name;

\echo ''
\echo '4. Freshness: rows and last content change per TVIEW (updated_at moves only when a row changes)'
SELECT pg_catalog.format(
           'SELECT %L AS tview, count(*) AS rows, max(updated_at) AS last_change, '
           'pg_catalog.now() - max(updated_at) AS since_last_change FROM %I.%I',
           r.schema || '.' || r.name, r.schema, r.name)
FROM tviews.registry r
WHERE r.schema IS NOT NULL
ORDER BY r.schema, r.name
\gexec

\echo ''
\echo '5. Physical health and warnings (sizes, HOT ratio, dead tuples, missing indexes, fan-out)'
SELECT entity, rows_estimate, pg_catalog.pg_size_pretty(heap_bytes) AS heap,
       round(hot_ratio::numeric, 2) AS hot_ratio, n_dead_tup, warnings
FROM tviews.pg_tviews_profile()
ORDER BY entity;

\echo ''
\echo '6. Replication readiness (UNLOGGED TVIEWs are empty on a standby)'
SELECT * FROM tviews.pg_tviews_replication_status() ORDER BY entity;

\echo ''
\echo '7. Refresh activity per TVIEW since the server started or the last reset'
\echo '   (tviews.stats needs shared_preload_libraries = ''pg_tviews'')'
SELECT entity, view_recomputes, noop_skipped, rows_written, rows_deleted,
       full_refreshes, round(refresh_ms::numeric, 1) AS refresh_ms, stats_reset, untracked
FROM tviews.stats
ORDER BY entity;

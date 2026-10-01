-- pg_tviews post-upgrade validation
-- Run after ALTER EXTENSION pg_tviews UPDATE (and pg_tviews_reregister_all()):
--   psql -X -v ON_ERROR_STOP=1 -d <database> -f docs/operations/upgrade/scripts/post-upgrade-validation.sql
--
-- Read-only, except step 4, which writes nothing either: it compares each TVIEW
-- with its backing view.

\echo '=== pg_tviews post-upgrade validation ==='

\echo ''
\echo '1. PostgreSQL and pg_tviews versions (library and catalog must match)'
SELECT pg_catalog.current_setting('server_version') AS postgresql,
       tviews.pg_tviews_version() AS library,
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews') AS extension;
SELECT status, message FROM tviews.pg_tviews_health_check() WHERE component = 'catalog';

\echo ''
\echo '2. TVIEWs still to re-register (run SELECT * FROM tviews.pg_tviews_reregister_all())'
SELECT schema, name FROM tviews.registry WHERE needs_reregister ORDER BY schema, name;

\echo ''
\echo '3. Everything the health check reports'
SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check()
ORDER BY CASE severity WHEN 'error' THEN 1 WHEN 'warning' THEN 2 ELSE 3 END, component;

\echo ''
\echo '4. Each TVIEW against its backing view (rows that differ; expect 0)'
SELECT pg_catalog.format(
           'SELECT %L AS tview, count(*) AS rows_differing FROM ('
           '(SELECT %I, data FROM %I.%I EXCEPT SELECT %I, data FROM %I.%I) '
           'UNION ALL (SELECT %I, data FROM %I.%I EXCEPT SELECT %I, data FROM %I.%I)) d',
           r.schema || '.' || r.name,
           'pk_' || r.entity, r.schema, r.name, 'pk_' || r.entity, r.schema, 'v_' || r.entity,
           'pk_' || r.entity, r.schema, 'v_' || r.entity, 'pk_' || r.entity, r.schema, r.name)
FROM tviews.registry r
WHERE r.schema IS NOT NULL AND r.view IS NOT NULL
ORDER BY r.schema, r.name
\gexec

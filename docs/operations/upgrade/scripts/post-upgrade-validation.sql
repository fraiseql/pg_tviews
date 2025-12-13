-- pg_tviews Post-Upgrade Validation Script
-- Comprehensive verification after upgrade operations
-- Run: psql -f docs/operations/upgrade/scripts/post-upgrade-validation.sql

\echo '=== pg_tviews Post-Upgrade Validation ==='
\echo 'Timestamp:' :DATE
\echo ''

-- Check 1: PostgreSQL accessibility
\echo '1. PostgreSQL Status:'
SELECT
    'PostgreSQL version' as check_item,
    version() as result,
    CASE
        WHEN version() LIKE '%PostgreSQL 15.%' THEN 'EXPECTED (15.x)'
        WHEN version() LIKE '%PostgreSQL 16.%' THEN 'EXPECTED (16.x)'
        WHEN version() LIKE '%PostgreSQL 17.%' THEN 'EXPECTED (17.x)'
        ELSE 'UNEXPECTED VERSION'
    END as status;

-- Check 2: pg_tviews extension
\echo ''
\echo '2. pg_tviews Extension Status:'
SELECT
    extname as extension_name,
    extversion as version,
    CASE
        WHEN extversion LIKE '0.%' THEN 'VALID (0.x series)'
        WHEN extversion LIKE '1.%' THEN 'VALID (1.x series)'
        ELSE 'UNKNOWN VERSION'
    END as status
FROM pg_extension
WHERE extname = 'pg_tviews';

-- Check extension functions
SELECT
    'Extension functions available' as check_item,
    COUNT(*) as function_count,
    CASE
        WHEN COUNT(*) > 10 THEN 'GOOD (functions present)'
        ELSE 'CONCERNING (few functions)'
    END as status
FROM pg_proc
WHERE proname LIKE 'pg_tviews%';

-- Check 3: TVIEW metadata integrity
\echo ''
\echo '3. TVIEW Metadata Integrity:'
SELECT
    'TVIEWs in metadata' as check_item,
    COUNT(*) as tview_count,
    CASE
        WHEN COUNT(*) > 0 THEN 'GOOD (TVIEWs present)'
        ELSE 'CRITICAL (no TVIEWs found)'
    END as status
FROM pg_tviews_metadata;

-- Check for TVIEW accessibility
\echo ''
\echo '4. TVIEW Accessibility Test:'
DO $$
DECLARE
    tview_record RECORD;
    accessible_count INTEGER := 0;
    total_count INTEGER := 0;
BEGIN
    FOR tview_record IN SELECT entity_name FROM pg_tviews_metadata LIMIT 5 LOOP
        BEGIN
            EXECUTE 'SELECT COUNT(*) FROM ' || tview_record.entity_name || ' LIMIT 1';
            accessible_count := accessible_count + 1;
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'TVIEW % not accessible: %', tview_record.entity_name, SQLERRM;
        END;
        total_count := total_count + 1;
    END LOOP;

    RAISE NOTICE 'TVIEW accessibility: %/% accessible', accessible_count, total_count;

    IF accessible_count = total_count THEN
        RAISE NOTICE '✅ All tested TVIEWs are accessible';
    ELSIF accessible_count > 0 THEN
        RAISE NOTICE '⚠️ Some TVIEWs accessible, others not';
    ELSE
        RAISE EXCEPTION '❌ No TVIEWs are accessible';
    END IF;
END $$;

-- Check 4: Refresh functionality
\echo ''
\echo '5. Refresh Functionality Test:'
-- Test basic refresh operation (use a test TVIEW if available)
DO $$
DECLARE
    test_tview TEXT;
    refresh_result TEXT;
BEGIN
    -- Find a TVIEW to test with
    SELECT entity_name INTO test_tview
    FROM pg_tviews_metadata
    WHERE last_error IS NULL
    LIMIT 1;

    IF test_tview IS NOT NULL THEN
        -- Try refresh operation
        BEGIN
            EXECUTE 'SELECT pg_tviews_refresh($1)' INTO refresh_result USING test_tview;
            RAISE NOTICE '✅ Refresh test successful on TVIEW: % (result: %)', test_tview, refresh_result;
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE '❌ Refresh test failed on TVIEW %: %', test_tview, SQLERRM;
        END;
    ELSE
        RAISE NOTICE '⚠️ No healthy TVIEWs available for refresh testing';
    END IF;
END $$;

-- Check 5: Queue system
\echo ''
\echo '6. Queue System Status:'
SELECT
    'Queue items' as check_item,
    COUNT(*) as total_items,
    COUNT(*) FILTER (WHERE processed_at IS NULL) as pending_items,
    CASE
        WHEN COUNT(*) FILTER (WHERE processed_at IS NULL) < 100 THEN 'GOOD (normal queue size)'
        WHEN COUNT(*) FILTER (WHERE processed_at IS NULL) < 1000 THEN 'WARNING (high queue)'
        ELSE 'CRITICAL (excessive queue)'
    END as status
FROM pg_tviews_queue;

-- Check for stuck items
SELECT
    'Stuck queue items (>1 hour)' as check_item,
    COUNT(*) as stuck_count,
    CASE
        WHEN COUNT(*) = 0 THEN 'GOOD (no stuck items)'
        ELSE 'WARNING (stuck items detected)'
    END as status
FROM pg_tviews_queue
WHERE processed_at IS NULL
  AND created_at < NOW() - INTERVAL '1 hour';

-- Check 6: Performance validation
\echo ''
\echo '7. Performance Validation:'
SELECT
    'Average refresh time' as metric,
    ROUND(AVG(last_refresh_duration_ms), 0) as avg_ms,
    CASE
        WHEN AVG(last_refresh_duration_ms) < 5000 THEN 'GOOD (<5s)'
        WHEN AVG(last_refresh_duration_ms) < 30000 THEN 'ACCEPTABLE (5-30s)'
        ELSE 'SLOW (>30s - investigate)'
    END as status
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '1 hour';

-- Check error rates
SELECT
    'Error rate (last 24h)' as metric,
    ROUND(
        COUNT(*) FILTER (WHERE last_error IS NOT NULL)::numeric /
        NULLIF(COUNT(*), 0) * 100, 2
    ) as error_percentage,
    CASE
        WHEN COUNT(*) FILTER (WHERE last_error IS NOT NULL) = 0 THEN 'EXCELLENT (0% errors)'
        WHEN COUNT(*) FILTER (WHERE last_error IS NOT NULL)::numeric / NULLIF(COUNT(*), 0) < 0.05 THEN 'GOOD (<5% errors)'
        WHEN COUNT(*) FILTER (WHERE last_error IS NOT NULL)::numeric / NULLIF(COUNT(*), 0) < 0.20 THEN 'ACCEPTABLE (5-20% errors)'
        ELSE 'CONCERNING (>20% errors)'
    END as status
FROM pg_tviews_metadata;

-- Check 7: System resource impact
\echo ''
\echo '8. System Resource Check:'
SELECT
    'Active connections' as metric,
    COUNT(*) as current_count,
    (SELECT setting FROM pg_settings WHERE name = 'max_connections') as max_allowed,
    CASE
        WHEN COUNT(*) < (SELECT setting FROM pg_settings WHERE name = 'max_connections')::integer * 0.8 THEN 'GOOD (normal usage)'
        ELSE 'HIGH (near connection limit)'
    END as status
FROM pg_stat_activity;

-- Check database size and growth
SELECT
    'Database size' as metric,
    pg_size_pretty(pg_database_size(current_database())) as size,
    'Reference only - monitor for unexpected growth' as notes
FROM pg_stat_bgwriter;

-- Check 8: Configuration validation
\echo ''
\echo '9. Configuration Validation:'
SELECT
    'pg_tviews settings' as check_item,
    COUNT(*) as settings_count,
    CASE
        WHEN COUNT(*) > 0 THEN 'GOOD (settings present)'
        ELSE 'INFO (no custom settings)'
    END as status
FROM pg_settings
WHERE name LIKE '%tview%' OR name LIKE '%refresh%';

-- Check 9: Final health assessment
\echo ''
\echo '10. Final Health Assessment:'

-- Comprehensive health check
DO $$
DECLARE
    tview_count INTEGER;
    healthy_count INTEGER;
    error_count INTEGER;
    pending_queue INTEGER;
    assessment TEXT := '';
BEGIN
    SELECT COUNT(*) INTO tview_count FROM pg_tviews_metadata;
    SELECT COUNT(*) INTO healthy_count FROM pg_tviews_metadata WHERE last_error IS NULL;
    SELECT COUNT(*) INTO error_count FROM pg_tviews_metadata WHERE last_error IS NOT NULL;
    SELECT COUNT(*) INTO pending_queue FROM pg_tviews_queue WHERE processed_at IS NULL;

    assessment := assessment || 'TVIEWs: ' || healthy_count || '/' || tview_count || ' healthy';

    IF error_count > 0 THEN
        assessment := assessment || ', ' || error_count || ' with errors';
    END IF;

    IF pending_queue > 100 THEN
        assessment := assessment || ', ' || pending_queue || ' queued items';
    END IF;

    RAISE NOTICE 'Post-upgrade assessment: %', assessment;

    -- Overall status
    IF error_count = 0 AND pending_queue < 100 AND healthy_count = tview_count THEN
        RAISE NOTICE '✅ OVERALL STATUS: EXCELLENT - Upgrade successful';
    ELSIF error_count = 0 AND healthy_count >= tview_count * 0.9 THEN
        RAISE NOTICE '✅ OVERALL STATUS: GOOD - Minor issues, upgrade successful';
    ELSIF healthy_count >= tview_count * 0.5 THEN
        RAISE NOTICE '⚠️ OVERALL STATUS: CONCERNING - Significant issues, monitor closely';
    ELSE
        RAISE EXCEPTION '❌ OVERALL STATUS: CRITICAL - Upgrade may have failed, consider rollback';
    END IF;
END $$;

\echo ''
\echo '=== Post-Upgrade Validation Complete ==='
\echo 'Review results above and take appropriate action.'
\echo 'If issues found, consider rollback procedures.'
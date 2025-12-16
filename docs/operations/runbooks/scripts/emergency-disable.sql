-- pg_tviews Emergency Disable Script
-- Emergency procedures for disabling TVIEW operations during crisis
-- USE WITH EXTREME CAUTION - This stops all TVIEW functionality
-- Run: psql -f docs/operations/runbooks/scripts/emergency-disable.sql

\echo '=== EMERGENCY TVIEW DISABLE ==='
\echo '⚠️  WARNING: This will stop ALL TVIEW operations!'
\echo 'Timestamp:' :DATE
\echo ''

-- Pre-disable assessment
\echo '1. Current System State:'
SELECT
    COUNT(*) as active_tviews,
    COUNT(*) FILTER (WHERE last_refreshed > NOW() - INTERVAL '5 minutes') as recently_active,
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) as pending_queue_items,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE query LIKE '%tview%' OR query LIKE '%refresh%') as active_tview_queries
FROM pg_tviews_metadata;

\echo ''
\echo '2. Active TVIEW-Related Queries:'
SELECT
    pid,
    usename,
    client_addr,
    query_start,
    LEFT(query, 80) as query_preview
FROM pg_stat_activity
WHERE query LIKE '%tview%' OR query LIKE '%refresh%'
  AND state = 'active'
ORDER BY query_start;

\echo ''
\echo '⚠️  EMERGENCY CONFIRMATION REQUIRED ⚠️'
\echo ''
\echo 'This script will:'
\echo '1. Cancel all active TVIEW refresh operations'
\echo '2. Disable automatic queue processing'
\echo '3. Mark all TVIEWs as temporarily unavailable'
\echo '4. Clear pending refresh queue'
\echo ''
\echo 'To proceed, set the confirmation variable:'
\echo '\set CONFIRM_DISABLE '\''YES_I_UNDERSTAND_THE_RISKS'\'''
\echo ''
\echo 'Then run the rest of this script.'

-- Emergency confirmation check
DO $$
BEGIN
    IF current_setting('CONFIRM_DISABLE', true) != 'YES_I_UNDERSTAND_THE_RISKS' THEN
        RAISE EXCEPTION 'Emergency disable not confirmed. Set CONFIRM_DISABLE to proceed.';
    END IF;

    RAISE NOTICE '✅ Emergency disable confirmed. Proceeding...';
END $$;

\echo ''
\echo '3. Step 1: Cancel Active TVIEW Operations'

-- Cancel active TVIEW queries (be very careful with this)
DO $$
DECLARE
    cancelled_count INTEGER := 0;
    r RECORD;
BEGIN
    FOR r IN
        SELECT pid, query
        FROM pg_stat_activity
        WHERE (query LIKE '%tview%' OR query LIKE '%refresh%')
          AND state = 'active'
          AND pid != pg_backend_pid()  -- Don't cancel ourselves
    LOOP
        BEGIN
            PERFORM pg_cancel_backend(r.pid);
            cancelled_count := cancelled_count + 1;
            RAISE NOTICE 'Cancelled TVIEW query (PID %): %', r.pid, LEFT(r.query, 50);
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'Failed to cancel PID %: %', r.pid, SQLERRM;
        END;
    END LOOP;

    RAISE NOTICE 'Cancelled % active TVIEW operations', cancelled_count;
END $$;

\echo ''
\echo '4. Step 2: Clear Pending Refresh Queue'

-- Clear the queue (this will lose pending refreshes)
SELECT
    'Queue items before cleanup' as status,
    COUNT(*) as count
FROM pg_tviews_queue
WHERE processed_at IS NULL;

-- Actually clear the queue
DELETE FROM pg_tviews_queue
WHERE processed_at IS NULL;

SELECT
    'Queue items after cleanup' as status,
    COUNT(*) as count
FROM pg_tviews_queue
WHERE processed_at IS NULL;

\echo ''
\echo '5. Step 3: Mark TVIEWs as Temporarily Disabled'

-- Add a marker to indicate emergency disable (if your system supports it)
-- This is conceptual - adjust based on your implementation
DO $$
BEGIN
    -- Example: Add emergency flag to metadata
    -- This assumes you have an emergency_disable column
    -- If not, you might need to rename tables or use other mechanisms

    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'pg_tviews_metadata'
          AND column_name = 'emergency_disabled'
    ) THEN
        UPDATE pg_tviews_metadata
        SET emergency_disabled = true,
            emergency_disable_time = NOW(),
            emergency_disable_reason = 'Emergency system disable';

        RAISE NOTICE 'Marked all TVIEWs as emergency disabled';
    ELSE
        RAISE NOTICE 'Emergency disable column not available - manual intervention required';
    END IF;
END $$;

\echo ''
\echo '6. Step 4: Disable Automatic Processing (if applicable)'

-- If you have background processing, disable it
-- This is system-specific - adjust for your deployment

DO $$
BEGIN
    -- Example: Disable via settings (if supported)
    -- PERFORM pg_tviews_set_setting('auto_refresh_enabled', 'false');

    RAISE NOTICE 'Automatic TVIEW processing disabled (manual configuration required)';
END $$;

\echo ''
\echo '7. Emergency Disable Complete'
\echo ''
\echo '📋 IMMEDIATE ACTION ITEMS:'
\echo '1. Notify application teams of TVIEW unavailability'
\echo '2. Redirect read queries to source tables if possible'
\echo '3. Monitor system for continued issues'
\echo '4. Schedule TVIEW re-enable procedure'
\echo ''
\echo '🔄 TO RE-ENABLE TVIEWs:'
\echo '1. Resolve root cause of emergency'
\echo '2. Run re-enable procedure (reverse of this script)'
\echo '3. Validate TVIEW functionality'
\echo '4. Monitor for several hours post-restore'
\echo ''
\echo '📞 CONTACT INFORMATION:'
\echo '- Database Team: [contact info]'
\echo '- Application Team: [contact info]'
\echo '- On-call Engineer: [contact info]'

-- Final status
\echo ''
\echo '=== EMERGENCY DISABLE COMPLETE ==='
\echo 'Timestamp:' :DATE
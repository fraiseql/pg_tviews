-- Test that pg_tviews hooks work correctly with PgBouncer

-- Setup logging
CREATE TABLE IF NOT EXISTS hook_log (
    ts TIMESTAMPTZ DEFAULT NOW(),
    event TEXT,
    details TEXT
);

-- Test DISCARD ALL handling
BEGIN;
INSERT INTO tb_pgbouncer_test (data) VALUES ('hook-test');
INSERT INTO hook_log (event, details)
  SELECT 'queue_before_discard', jsonb_array_length(pg_tviews_debug_queue())::TEXT;
COMMIT;

DISCARD ALL;

-- After DISCARD ALL, queue should be empty
INSERT INTO hook_log (event, details)
  SELECT 'queue_after_discard', jsonb_array_length(pg_tviews_debug_queue())::TEXT;

-- Verify
SELECT event, details FROM hook_log ORDER BY ts;

-- Expected:
-- queue_before_discard | 1
-- queue_after_discard  | 0
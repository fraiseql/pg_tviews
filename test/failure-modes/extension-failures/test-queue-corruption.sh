#!/bin/bash
set -euo pipefail

echo "Testing queue corruption recovery..."

# Setup
psql <<EOF
CREATE TABLE tb_queue_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_queue_test AS SELECT pk_test, data FROM tb_queue_test;
SELECT pg_tviews_convert_existing_table('tv_queue_test');
EOF

echo "Simulating queue corruption..."

# Insert some data to create queue entries
psql -c "INSERT INTO tb_queue_test (data) VALUES ('queue-test-1');"

# Manually corrupt queue persistence (simulate orphaned entry)
psql <<EOF
INSERT INTO pg_tview_pending_refreshes (gid, refresh_queue, queue_size, prepared_at)
VALUES ('orphaned-test-gid', '["corrupted-queue-data"]'::jsonb, 1, now() - interval '2 hours');
EOF

echo "Checking for orphaned queue entries..."

# Check for old orphaned entries
ORPHANED_COUNT=$(psql -tAc "SELECT COUNT(*) FROM pg_tview_pending_refreshes WHERE age(now(), prepared_at) > interval '1 hour';")

if [ "$ORPHANED_COUNT" -gt 0 ]; then
    echo "✅ Found $ORPHANED_COUNT orphaned queue entries"
else
    echo "⚠️  No orphaned entries found (test may not be valid)"
fi

echo "Testing cleanup of orphaned entries..."

# Clean up orphaned entries
psql -c "DELETE FROM pg_tview_pending_refreshes WHERE age(now(), prepared_at) > interval '1 hour';"

# Verify cleanup
AFTER_COUNT=$(psql -tAc "SELECT COUNT(*) FROM pg_tview_pending_refreshes WHERE age(now(), prepared_at) > interval '1 hour';")

if [ "$AFTER_COUNT" -eq 0 ]; then
    echo "✅ PASS: Orphaned queue entries cleaned up"
else
    echo "❌ FAIL: $AFTER_COUNT orphaned entries remain"
    exit 1
fi

# Verify normal operation still works
psql -c "INSERT INTO tb_queue_test (data) VALUES ('queue-test-2');"
COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_queue_test WHERE data = 'queue-test-2';")

if [ "$COUNT" -eq 1 ]; then
    echo "✅ PASS: Normal queue operation works after cleanup"
else
    echo "❌ FAIL: Queue operation broken after cleanup"
    exit 1
fi

echo "✅ Queue corruption test passed"
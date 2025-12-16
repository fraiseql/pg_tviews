#!/bin/bash
set -euo pipefail

echo "Testing crash recovery..."

# Setup
psql <<EOF
CREATE TABLE tb_crash_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_crash_test AS SELECT pk_test, data FROM tb_crash_test;
SELECT pg_tviews_convert_existing_table('tv_crash_test');
EOF

# Insert some data
psql -c "INSERT INTO tb_crash_test (data) SELECT 'row-' || i FROM generate_series(1, 1000) i;"

echo "Simulating crash during refresh..."

# Start a long-running refresh in background
psql <<EOF &
BEGIN;
INSERT INTO tb_crash_test (data) VALUES ('crash-test');
-- Simulate long-running transaction
SELECT pg_sleep(5);
COMMIT;
EOF

# Wait a bit, then simulate crash
sleep 2
sudo systemctl restart postgresql

# Wait for PostgreSQL to come back up
sleep 5
until pg_isready; do sleep 1; done

echo "Checking recovery..."

# Verify TVIEW is consistent
BACKING_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tb_crash_test;")
TVIEW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_crash_test;")

echo "Backing table: $BACKING_COUNT rows"
echo "TVIEW: $TVIEW_COUNT rows"

# After recovery, TVIEW should be consistent
# The crashed transaction should have rolled back
if [ "$TVIEW_COUNT" -eq 1000 ]; then
    echo "✅ PASS: TVIEW recovered correctly after crash"
else
    echo "❌ FAIL: TVIEW in inconsistent state"
    exit 1
fi

# Check for orphaned queue entries
QUEUE_SIZE=$(psql -tAc "SELECT jsonb_array_length(pg_tviews_debug_queue());")
if [ "$QUEUE_SIZE" -eq 0 ]; then
    echo "✅ PASS: No orphaned queue entries"
else
    echo "⚠️  WARNING: Queue has $QUEUE_SIZE orphaned entries"
fi

echo "✅ Crash recovery test passed"
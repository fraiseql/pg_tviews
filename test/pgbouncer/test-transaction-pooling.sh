#!/bin/bash
set -euo pipefail

export PGHOST=localhost
export PGPORT=6432  # PgBouncer port

echo "Testing transaction pooling mode..."

# Setup test table
psql -c "CREATE TABLE IF NOT EXISTS tb_pgbouncer_test (pk_test SERIAL PRIMARY KEY, data TEXT);"
psql -c "DROP TABLE IF EXISTS tv_pgbouncer_test CASCADE;"
psql -c "CREATE TABLE tv_pgbouncer_test AS SELECT pk_test, data FROM tb_pgbouncer_test;"
psql -c "SELECT pg_tviews_convert_existing_table('tv_pgbouncer_test');"

echo "Test 1: DISCARD ALL clears queue"

# Session 1: Add to queue
psql <<EOF
BEGIN;
INSERT INTO tb_pgbouncer_test (data) VALUES ('test-1');
-- Queue should have entry
SELECT jsonb_array_length(pg_tviews_debug_queue()) as queue_size;
COMMIT;
EOF

# Session 2: Check queue (should be cleared by DISCARD ALL)
QUEUE_SIZE=$(psql -tAc "SELECT jsonb_array_length(pg_tviews_debug_queue());")

if [ "$QUEUE_SIZE" -eq 0 ]; then
    echo "✅ PASS: Queue cleared after DISCARD ALL"
else
    echo "❌ FAIL: Queue still has $QUEUE_SIZE entries"
    exit 1
fi

echo "Test 2: No cross-transaction contamination"

# Run multiple transactions, verify isolation
for i in {1..10}; do
    psql <<EOF
BEGIN;
INSERT INTO tb_pgbouncer_test (data) VALUES ('txn-$i');
COMMIT;
EOF
done

# Verify all 11 rows exist (1 from test 1 + 10 new)
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_pgbouncer_test;")
if [ "$ROW_COUNT" -eq 11 ]; then
    echo "✅ PASS: All transactions processed correctly"
else
    echo "❌ FAIL: Expected 11 rows, got $ROW_COUNT"
    exit 1
fi

echo "✅ Transaction pooling tests passed"
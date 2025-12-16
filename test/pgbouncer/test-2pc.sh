#!/bin/bash
set -euo pipefail

export PGHOST=localhost
export PGPORT=6432

echo "Testing 2PC edge cases..."

# Setup
psql <<EOF
CREATE TABLE IF NOT EXISTS tb_2pc (pk_test SERIAL PRIMARY KEY, data TEXT);
DROP TABLE IF EXISTS tv_2pc CASCADE;
CREATE TABLE tv_2pc AS SELECT pk_test, data FROM tb_2pc;
SELECT pg_tviews_convert_existing_table('tv_2pc');
EOF

echo "Test 1: Prepare → Commit"

psql <<EOF
BEGIN;
INSERT INTO tb_2pc (data) VALUES ('2pc-test-1');
PREPARE TRANSACTION 'test_2pc_1';
EOF

# Check queue persistence
QUEUE_PERSISTED=$(psql -tAc "SELECT COUNT(*) FROM pg_tview_pending_refreshes WHERE gid = 'test_2pc_1';")
if [ "$QUEUE_PERSISTED" -eq 0 ]; then
    echo "❌ FAIL: Queue not persisted for prepared transaction"
    exit 1
fi

# Commit
psql -c "COMMIT PREPARED 'test_2pc_1';"

# Verify refresh happened
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_2pc WHERE data = '2pc-test-1';")
if [ "$ROW_COUNT" -eq 1 ]; then
    echo "✅ PASS: 2PC commit refreshed TVIEW"
else
    echo "❌ FAIL: TVIEW not refreshed after 2PC commit"
    exit 1
fi

echo "Test 2: Prepare → Rollback"

psql <<EOF
BEGIN;
INSERT INTO tb_2pc (data) VALUES ('2pc-test-rollback');
PREPARE TRANSACTION 'test_2pc_rollback';
EOF

psql -c "ROLLBACK PREPARED 'test_2pc_rollback';"

# Verify no refresh
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_2pc WHERE data = '2pc-test-rollback';")
if [ "$ROW_COUNT" -eq 0 ]; then
    echo "✅ PASS: 2PC rollback did not refresh TVIEW"
else
    echo "❌ FAIL: TVIEW incorrectly refreshed after rollback"
    exit 1
fi

echo "Test 3: Multiple prepared transactions"

# Create 5 prepared transactions
for i in {1..5}; do
    psql <<EOF
BEGIN;
INSERT INTO tb_2pc (data) VALUES ('2pc-multi-$i');
PREPARE TRANSACTION 'test_2pc_multi_$i';
EOF
done

# Verify all persisted
PERSISTED_COUNT=$(psql -tAc "SELECT COUNT(*) FROM pg_tview_pending_refreshes WHERE gid LIKE 'test_2pc_multi_%';")
if [ "$PERSISTED_COUNT" -eq 5 ]; then
    echo "✅ PASS: All 5 prepared transactions persisted"
else
    echo "❌ FAIL: Expected 5 persisted, got $PERSISTED_COUNT"
    exit 1
fi

# Commit all
for i in {1..5}; do
    psql -c "COMMIT PREPARED 'test_2pc_multi_$i';"
done

# Verify all refreshed
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_2pc WHERE data LIKE '2pc-multi-%';")
if [ "$ROW_COUNT" -eq 5 ]; then
    echo "✅ PASS: All prepared transactions committed and refreshed"
else
    echo "❌ FAIL: Expected 5 rows, got $ROW_COUNT"
    exit 1
fi

echo "✅ 2PC validation tests passed"
#!/bin/bash
set -euo pipefail

export PGHOST=localhost
export PGPORT=6432

echo "Testing 2PC edge cases..."

# Setup
psql <<EOF
DROP TABLE IF EXISTS tv_twopc CASCADE;
CREATE TABLE IF NOT EXISTS tb_twopc (pk_twopc SERIAL PRIMARY KEY, id UUID NOT NULL DEFAULT gen_random_uuid(), label TEXT);
CREATE TABLE tv_twopc AS SELECT pk_twopc, id, jsonb_build_object('label', label) AS data FROM tb_twopc;
EOF

echo "Test 1: Prepare → Commit"

psql <<EOF
BEGIN;
INSERT INTO tb_twopc (label) VALUES ('2pc-test-1');
PREPARE TRANSACTION 'test_2pc_1';
EOF

# Not visible before COMMIT PREPARED
BEFORE=$(psql -tAc "SELECT COUNT(*) FROM tv_twopc WHERE data->>'label' = '2pc-test-1';")
if [ "$BEFORE" -ne 0 ]; then
    echo "❌ FAIL: TVIEW change visible before COMMIT PREPARED"
    exit 1
fi

# Commit
psql -c "COMMIT PREPARED 'test_2pc_1';"

# Verify refresh happened
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_twopc WHERE data->>'label' = '2pc-test-1';")
if [ "$ROW_COUNT" -eq 1 ]; then
    echo "✅ PASS: 2PC commit refreshed TVIEW"
else
    echo "❌ FAIL: TVIEW not refreshed after 2PC commit"
    exit 1
fi

echo "Test 2: Prepare → Rollback"

psql <<EOF
BEGIN;
INSERT INTO tb_twopc (label) VALUES ('2pc-test-rollback');
PREPARE TRANSACTION 'test_2pc_rollback';
EOF

psql -c "ROLLBACK PREPARED 'test_2pc_rollback';"

# Verify no refresh
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_twopc WHERE data->>'label' = '2pc-test-rollback';")
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
INSERT INTO tb_twopc (label) VALUES ('2pc-multi-$i');
PREPARE TRANSACTION 'test_2pc_multi_$i';
EOF
done

# Verify all prepared
PREPARED_COUNT=$(psql -tAc "SELECT COUNT(*) FROM pg_prepared_xacts WHERE gid LIKE 'test_2pc_multi_%';")
if [ "$PREPARED_COUNT" -eq 5 ]; then
    echo "✅ PASS: All 5 transactions prepared"
else
    echo "❌ FAIL: Expected 5 prepared transactions, got $PREPARED_COUNT"
    exit 1
fi

# Commit all
for i in {1..5}; do
    psql -c "COMMIT PREPARED 'test_2pc_multi_$i';"
done

# Verify all refreshed
ROW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_twopc WHERE data->>'label' LIKE '2pc-multi-%';")
if [ "$ROW_COUNT" -eq 5 ]; then
    echo "✅ PASS: All prepared transactions committed and refreshed"
else
    echo "❌ FAIL: Expected 5 rows, got $ROW_COUNT"
    exit 1
fi

echo "✅ 2PC validation tests passed"
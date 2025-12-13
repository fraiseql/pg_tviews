#!/bin/bash
set -euo pipefail

echo "Testing SQL injection vulnerabilities..."

# Setup
psql <<EOF
CREATE TABLE tb_inject_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_inject_test AS SELECT pk_test, data FROM tb_inject_test;
SELECT pg_tviews_convert_existing_table('tv_inject_test');
EOF

echo "Test 1: Entity name injection"

# Try to inject SQL via entity name
set +e
psql -c "SELECT pg_tviews_convert_existing_table('tv_inject_test; DROP TABLE tb_inject_test; --');" 2>&1 | tee /tmp/inject-test.txt
RESULT=$?
set -e

# Should fail safely
if [ $RESULT -ne 0 ]; then
    echo "✅ PASS: SQL injection blocked"
    grep -qi "invalid" /tmp/inject-test.txt && echo "✅ Proper error message"
else
    # Check if tb_inject_test still exists
    if psql -c "SELECT 1 FROM tb_inject_test LIMIT 1;" &>/dev/null; then
        echo "✅ PASS: Table not dropped, injection failed"
    else
        echo "❌ CRITICAL: SQL injection succeeded - table dropped!"
        exit 1
    fi
fi

echo "Test 2: Column name injection"

# Try to inject via JSONB field
set +e
psql -c "INSERT INTO tb_inject_test (data) VALUES ('test');"
psql -c "SELECT pg_tviews_refresh('tv_inject_test', jsonb_fields => ARRAY['data); DROP TABLE tb_inject_test; --']);" 2>&1 | tee /tmp/column-inject.txt
RESULT=$?
set -e

if [ $RESULT -ne 0 ]; then
    echo "✅ PASS: Column injection blocked"
fi

echo "Test 3: Batch SQL injection"

# Test if batch operations properly escape
psql <<EOF
-- Create legitimate TVIEW
CREATE TABLE tv_batch_test AS SELECT 1 as id;
SELECT pg_tviews_convert_existing_table('tv_batch_test');

-- Try batch refresh with malicious entity name
SELECT pg_tviews_refresh_batch(ARRAY['tv_batch_test', 'evil''; DROP TABLE tb_inject_test; --']);
EOF

# Verify table still exists
if psql -c "SELECT 1 FROM tb_inject_test LIMIT 1;" &>/dev/null; then
    echo "✅ PASS: Batch injection blocked"
else
    echo "❌ CRITICAL: Batch SQL injection succeeded!"
    exit 1
fi

echo "✅ SQL injection tests passed"
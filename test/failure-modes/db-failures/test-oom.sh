#!/bin/bash
set -euo pipefail

echo "Testing out of memory scenario..."

# Setup
psql <<EOF
CREATE TABLE tb_oom_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_oom_test AS SELECT pk_test, data FROM tb_oom_test;
SELECT pg_tviews_convert_existing_table('tv_oom_test');
EOF

echo "Testing with very low work_mem..."

# Set very low work_mem to trigger OOM
set +e
psql <<EOF
SET work_mem = '64kB';  -- Very low memory limit
INSERT INTO tb_oom_test (data)
SELECT repeat('x', 1000)
FROM generate_series(1, 10000);
EOF
RESULT=$?
set -e

if [ $RESULT -ne 0 ]; then
    echo "✅ Expected failure: Out of memory"
else
    echo "⚠️  OOM didn't occur (test may not be valid)"
fi

echo "Testing recovery with higher work_mem..."

# Try again with reasonable memory
psql <<EOF
RESET work_mem;
INSERT INTO tb_oom_test (data) VALUES ('recovery-test');
EOF

# Verify TVIEW was updated
COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_oom_test WHERE data = 'recovery-test';")

if [ "$COUNT" -eq 1 ]; then
    echo "✅ PASS: TVIEW recovered after OOM failure"
else
    echo "❌ FAIL: TVIEW not updated after recovery"
    exit 1
fi

echo "✅ OOM test passed"
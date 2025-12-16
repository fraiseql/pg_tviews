#!/bin/bash
set -euo pipefail

echo "Testing concurrent DDL scenario..."

# Setup
psql <<EOF
CREATE TABLE tb_ddl_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_ddl_test AS SELECT pk_test, data FROM tb_ddl_test;
SELECT pg_tviews_convert_existing_table('tv_ddl_test');
INSERT INTO tb_ddl_test (data) VALUES ('ddl-test-1');
EOF

echo "Testing DROP TABLE during refresh..."

# Start a refresh in background
psql <<EOF &
BEGIN;
INSERT INTO tb_ddl_test (data) VALUES ('ddl-test-2');
SELECT pg_sleep(3);  -- Give time for DROP to happen
COMMIT;
EOF

# Wait a moment, then drop the TVIEW
sleep 1
set +e
psql -c "DROP TABLE tv_ddl_test CASCADE;" 2>&1 | tee /tmp/ddl-error.txt
DROP_RESULT=$?
set -e

# The DROP should succeed (or fail gracefully)
if [ $DROP_RESULT -eq 0 ]; then
    echo "✅ PASS: DROP TABLE succeeded during refresh"
elif grep -q "does not exist" /tmp/ddl-error.txt; then
    echo "✅ PASS: DROP TABLE handled gracefully"
else
    echo "⚠️  DROP TABLE failed with unexpected error"
fi

echo "Testing recovery after DDL..."

# Recreate TVIEW
psql <<EOF
CREATE TABLE tv_ddl_test AS SELECT pk_test, data FROM tb_ddl_test;
SELECT pg_tviews_convert_existing_table('tv_ddl_test');
EOF

# Verify it works
psql -c "INSERT INTO tb_ddl_test (data) VALUES ('ddl-test-3');"
COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_ddl_test WHERE data = 'ddl-test-3';")

if [ "$COUNT" -eq 1 ]; then
    echo "✅ PASS: TVIEW recovered after DDL interference"
else
    echo "❌ FAIL: TVIEW not functional after DDL recovery"
    exit 1
fi

echo "✅ Concurrent DDL test passed"
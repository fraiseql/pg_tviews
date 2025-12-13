#!/bin/bash
set -euo pipefail

echo "Testing circular dependency detection..."

# Setup
psql <<EOF
CREATE TABLE tb_a (pk_a SERIAL PRIMARY KEY, fk_b INT, data TEXT);
CREATE TABLE tb_b (pk_b SERIAL PRIMARY KEY, fk_a INT, data TEXT);

-- Create tv_a that references tb_b
CREATE TABLE tv_a AS
SELECT a.pk_a, a.data, b.data as b_data
FROM tb_a a
LEFT JOIN tb_b b ON a.fk_b = b.pk_b;

SELECT pg_tviews_convert_existing_table('tv_a');

-- Try to create tv_b that references tv_a (creates cycle)
CREATE TABLE tv_b AS
SELECT b.pk_b, b.data, a.data as a_data
FROM tb_b b
LEFT JOIN tv_a a ON b.fk_a = a.pk_a;
EOF

echo "Attempting to create circular dependency..."

set +e
psql -c "SELECT pg_tviews_convert_existing_table('tv_b');" 2>&1 | tee /tmp/circular-error.txt
RESULT=$?
set -e

if [ $RESULT -ne 0 ]; then
    echo "✅ PASS: Circular dependency detected and prevented"
    grep -q "circular" /tmp/circular-error.txt && echo "✅ Error message mentions 'circular'"
else
    echo "❌ FAIL: Circular dependency allowed"
    exit 1
fi

# Verify tv_a still works
psql -c "SELECT COUNT(*) FROM tv_a;" > /dev/null
echo "✅ PASS: Existing TVIEW (tv_a) still functional"

echo "✅ Circular dependency test passed"
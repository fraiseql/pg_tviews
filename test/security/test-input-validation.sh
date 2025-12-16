#!/bin/bash
set -euo pipefail

echo "Testing input validation..."

echo "Test 1: Invalid entity names"

INVALID_NAMES=(
    ""                      # Empty
    "a"                     # Too short
    "$(printf 'x%.0s' {1..256})"  # Too long
    "123_invalid"           # Starts with number
    "invalid-dash"          # Contains dash
    "invalid space"         # Contains space
    "invalid;drop"          # Contains semicolon
)

for name in "${INVALID_NAMES[@]}"; do
    set +e
    psql -c "SELECT pg_tviews_convert_existing_table('$name');" 2>/dev/null
    RESULT=$?
    set -e

    if [ $RESULT -ne 0 ]; then
        echo "✅ Rejected: '$name'"
    else
        echo "❌ FAIL: Accepted invalid name: '$name'"
        exit 1
    fi
done

echo "Test 2: Validate dependency depth limit"

# Create deep dependency chain
psql <<EOF
CREATE TABLE tb_depth_0 (pk INT PRIMARY KEY);
CREATE TABLE tv_depth_0 AS SELECT pk FROM tb_depth_0;
SELECT pg_tviews_convert_existing_table('tv_depth_0');
EOF

# Try to create 11 levels (exceeds limit of 10)
for i in {1..11}; do
    PREV=$((i-1))
    psql <<EOF
CREATE TABLE tb_depth_$i (pk INT PRIMARY KEY, fk INT);
CREATE TABLE tv_depth_$i AS
    SELECT d$i.pk, d$PREV.pk as prev_pk
    FROM tb_depth_$i d$i
    LEFT JOIN tv_depth_$PREV d$PREV ON d$i.fk = d$PREV.pk;
EOF

    set +e
    psql -c "SELECT pg_tviews_convert_existing_table('tv_depth_$i');" 2>&1 | tee /tmp/depth-test.txt
    RESULT=$?
    set -e

    if [ $i -gt 10 ] && [ $RESULT -ne 0 ]; then
        echo "✅ PASS: Dependency depth limit enforced at level $i"
        grep -qi "dependency depth" /tmp/depth-test.txt && echo "✅ Clear error message"
        break
    fi
done

echo "✅ Input validation tests passed"
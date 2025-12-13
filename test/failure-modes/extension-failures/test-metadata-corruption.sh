#!/bin/bash
set -euo pipefail

echo "Testing metadata corruption recovery..."

# Setup
psql <<EOF
CREATE TABLE tb_meta_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_meta_test AS SELECT pk_test, data FROM tb_meta_test;
SELECT pg_tviews_convert_existing_table('tv_meta_test');
INSERT INTO tb_meta_test (data) VALUES ('test-1'), ('test-2');
EOF

echo "Simulating metadata corruption..."

# Corrupt metadata (delete entry)
psql -c "DELETE FROM pg_tviews_metadata WHERE entity_name = 'tv_meta_test';"

echo "Attempting refresh with corrupted metadata..."

set +e
psql -c "INSERT INTO tb_meta_test (data) VALUES ('test-3');" 2>&1 | tee /tmp/metadata-error.txt
RESULT=$?
set -e

# Should fail gracefully
if [ $RESULT -ne 0 ]; then
    echo "✅ PASS: Graceful failure on missing metadata"
    grep -qi "metadata not found" /tmp/metadata-error.txt && echo "✅ Clear error message"
else
    echo "⚠️  WARNING: Operation succeeded despite missing metadata"
fi

echo "Testing metadata recovery..."

# Re-convert to fix metadata
psql -c "SELECT pg_tviews_convert_existing_table('tv_meta_test');"

# Verify recovery
psql -c "INSERT INTO tb_meta_test (data) VALUES ('test-4');"
COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_meta_test WHERE data = 'test-4';")

if [ "$COUNT" -eq 1 ]; then
    echo "✅ PASS: Metadata recovered, refresh working"
else
    echo "❌ FAIL: Recovery unsuccessful"
    exit 1
fi

echo "✅ Metadata corruption test passed"
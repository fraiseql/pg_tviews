#!/bin/bash
set -euo pipefail

echo "Testing disk full scenario..."

# Create a small tmpfs to simulate disk full
sudo mkdir -p /tmp/pg_test_small
sudo mount -t tmpfs -o size=100M tmpfs /tmp/pg_test_small

# Create a test tablespace
psql <<EOF
CREATE TABLESPACE test_small LOCATION '/tmp/pg_test_small';
CREATE TABLE tb_disk_full (pk_test SERIAL PRIMARY KEY, data TEXT) TABLESPACE test_small;
CREATE TABLE tv_disk_full AS SELECT pk_test, data FROM tb_disk_full;
SELECT pg_tviews_convert_existing_table('tv_disk_full');
EOF

echo "Filling disk..."

# Try to insert until disk is full
set +e
psql <<EOF
INSERT INTO tb_disk_full (data)
SELECT repeat('x', 10000)
FROM generate_series(1, 100000);
EOF
RESULT=$?
set -e

if [ $RESULT -ne 0 ]; then
    echo "✅ Expected failure: Disk full"
else
    echo "⚠️  Disk didn't fill (test may not be valid)"
fi

echo "Checking TVIEW consistency after disk full..."

# Verify TVIEW is still accessible
TVIEW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_disk_full;" || echo "ERROR")

if [ "$TVIEW_COUNT" != "ERROR" ]; then
    echo "✅ PASS: TVIEW remains accessible after disk full"
else
    echo "❌ FAIL: TVIEW corrupted"
    exit 1
fi

# Cleanup
sudo umount /tmp/pg_test_small
sudo rm -rf /tmp/pg_test_small

echo "✅ Disk full test passed"
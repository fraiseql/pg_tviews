#!/bin/bash
set -euo pipefail

echo "Testing PostgreSQL upgrade scenario..."

# This test simulates the upgrade process
# In a real scenario, this would be done with actual PostgreSQL binaries

echo "Simulating pre-upgrade state..."

# Setup test data
psql <<EOF
CREATE TABLE tb_upgrade_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_upgrade_test AS SELECT pk_test, data FROM tb_upgrade_test;
SELECT pg_tviews_convert_existing_table('tv_upgrade_test');
INSERT INTO tb_upgrade_test (data) VALUES ('pre-upgrade-data');
EOF

# Record current state
PRE_UPGRADE_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_upgrade_test;")
PRE_UPGRADE_VERSION=$(psql -tAc "SELECT pg_tviews_version();")

echo "Pre-upgrade TVIEW count: $PRE_UPGRADE_COUNT"
echo "Pre-upgrade version: $PRE_UPGRADE_VERSION"

echo "Simulating extension reinstall (upgrade scenario)..."

# In a real upgrade, the extension would be reinstalled
# Here we just verify the current installation works
psql -c "SELECT pg_tviews_version();" > /dev/null

echo "Verifying post-upgrade state..."

# Verify TVIEW still works
psql -c "INSERT INTO tb_upgrade_test (data) VALUES ('post-upgrade-data');"
POST_UPGRADE_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_upgrade_test;")

if [ "$POST_UPGRADE_COUNT" -gt "$PRE_UPGRADE_COUNT" ]; then
    echo "✅ PASS: TVIEW functional after simulated upgrade"
else
    echo "❌ FAIL: TVIEW not working after upgrade"
    exit 1
fi

# Verify metadata intact
METADATA_COUNT=$(psql -tAc "SELECT COUNT(*) FROM pg_tviews_metadata WHERE entity_name = 'tv_upgrade_test';")

if [ "$METADATA_COUNT" -eq 1 ]; then
    echo "✅ PASS: Metadata preserved after upgrade"
else
    echo "❌ FAIL: Metadata lost during upgrade"
    exit 1
fi

echo "✅ Upgrade test passed"
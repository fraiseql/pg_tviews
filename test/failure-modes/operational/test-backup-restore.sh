#!/bin/bash
set -euo pipefail

echo "Testing backup and restore scenario..."

# Setup test data
psql <<EOF
CREATE TABLE tb_backup_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_backup_test AS SELECT pk_test, data FROM tb_backup_test;
SELECT pg_tviews_convert_existing_table('tv_backup_test');
INSERT INTO tb_backup_test (data) VALUES ('backup-test-1'), ('backup-test-2');
EOF

echo "Creating backup..."

# Create a logical backup
BACKUP_FILE="/tmp/pg_tviews_backup_test.sql"
pg_dump --no-owner --no-privileges -f "$BACKUP_FILE"

echo "Simulating restore to new database..."

# Create a new database for restore test
psql -c "CREATE DATABASE pg_tviews_restore_test;" postgres

# Restore to new database
psql -d pg_tviews_restore_test -f "$BACKUP_FILE"

echo "Verifying restore..."

# Check if TVIEW was restored
RESTORE_COUNT=$(psql -d pg_tviews_restore_test -tAc "SELECT COUNT(*) FROM tv_backup_test;")

if [ "$RESTORE_COUNT" -eq 2 ]; then
    echo "✅ PASS: TVIEW data restored correctly"
else
    echo "❌ FAIL: Expected 2 rows, got $RESTORE_COUNT"
    exit 1
fi

# Check if metadata was restored
METADATA_COUNT=$(psql -d pg_tviews_restore_test -tAc "SELECT COUNT(*) FROM pg_tviews_metadata WHERE entity_name = 'tv_backup_test';")

if [ "$METADATA_COUNT" -eq 1 ]; then
    echo "✅ PASS: TVIEW metadata restored"
else
    echo "❌ FAIL: TVIEW metadata not restored"
    exit 1
fi

# Test that refresh still works after restore
psql -d pg_tviews_restore_test -c "INSERT INTO tb_backup_test (data) VALUES ('post-restore-test');"
POST_RESTORE_COUNT=$(psql -d pg_tviews_restore_test -tAc "SELECT COUNT(*) FROM tv_backup_test WHERE data = 'post-restore-test';")

if [ "$POST_RESTORE_COUNT" -eq 1 ]; then
    echo "✅ PASS: TVIEW refresh works after restore"
else
    echo "❌ FAIL: TVIEW refresh broken after restore"
    exit 1
fi

# Cleanup
psql -c "DROP DATABASE pg_tviews_restore_test;" postgres
rm -f "$BACKUP_FILE"

echo "✅ Backup/restore test passed"
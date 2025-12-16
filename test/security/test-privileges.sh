#!/bin/bash
set -euo pipefail

echo "Testing privilege escalation vectors..."

# Create non-superuser
psql <<EOF
DROP ROLE IF EXISTS tview_test_user;
CREATE ROLE tview_test_user LOGIN PASSWORD 'test';
GRANT CREATE ON DATABASE postgres TO tview_test_user;
EOF

echo "Test 1: Non-superuser cannot bypass RLS"

psql <<EOF
-- Create table with RLS
CREATE TABLE tb_rls_test (pk_test INT PRIMARY KEY, data TEXT, owner TEXT);
ALTER TABLE tb_rls_test ENABLE ROW LEVEL SECURITY;

CREATE POLICY rls_policy ON tb_rls_test
    USING (owner = current_user);

CREATE TABLE tv_rls_test AS SELECT pk_test, data FROM tb_rls_test;
SELECT pg_tviews_convert_existing_table('tv_rls_test');

-- Insert data as superuser
INSERT INTO tb_rls_test VALUES (1, 'secret', 'postgres');
INSERT INTO tb_rls_test VALUES (2, 'public', 'tview_test_user');
EOF

# Connect as test user
PGUSER=tview_test_user PGPASSWORD=test psql <<EOF
-- Should only see own data
SELECT * FROM tv_rls_test;
EOF

ROW_COUNT=$(PGUSER=tview_test_user PGPASSWORD=test psql -tAc "SELECT COUNT(*) FROM tv_rls_test;")

if [ "$ROW_COUNT" -eq 1 ]; then
    echo "✅ PASS: RLS enforced on TVIEW"
else
    echo "❌ FAIL: RLS bypassed (saw $ROW_COUNT rows, expected 1)"
    exit 1
fi

echo "Test 2: Non-superuser cannot modify metadata"

set +e
PGUSER=tview_test_user PGPASSWORD=test psql <<EOF
INSERT INTO pg_tviews_metadata (entity_name, backing_view, pk_column)
VALUES ('evil_view', 'pg_authid', 'oid');
EOF
RESULT=$?
set -e

if [ $RESULT -ne 0 ]; then
    echo "✅ PASS: Metadata table protected"
else
    echo "❌ FAIL: Non-superuser modified metadata"
    exit 1
fi

# Cleanup
psql -c "DROP ROLE tview_test_user;"

echo "✅ Privilege tests passed"
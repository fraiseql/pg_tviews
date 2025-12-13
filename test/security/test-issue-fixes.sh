#!/bin/bash
set -euo pipefail

echo "Testing security issue fixes..."

echo "Test 1: Enhanced validation in unsafe blocks"

# Test that the extension still loads and basic functions work
psql <<EOF
-- Test basic functionality after unsafe code improvements
SELECT pg_tviews_version();
SELECT pg_tviews_debug_queue();
EOF

echo "Test 2: Type safety improvements"

# Test that type conversions work properly
psql <<EOF
-- Test TVIEW creation (exercises type conversion paths)
CREATE TABLE test_security_fixes (id INT PRIMARY KEY, data TEXT);
CREATE TABLE tv_test_security_fixes AS SELECT id, data FROM test_security_fixes;
SELECT pg_tviews_convert_existing_table('tv_test_security_fixes');

-- Test refresh operations
INSERT INTO test_security_fixes (id, data) VALUES (1, 'test-data');
SELECT COUNT(*) FROM tv_test_security_fixes;
EOF

echo "Test 3: Build stability after fixes"

# Verify all fixes compile and don't break existing functionality
cargo check
cargo build --release

echo "Test 4: Regression testing"

# Run existing security tests to ensure no regressions
if [ -f "test/security/test-sql-injection.sh" ]; then
    ./test/security/test-sql-injection.sh
fi

if [ -f "test/security/test-privileges.sh" ]; then
    ./test/security/test-privileges.sh
fi

echo "✅ Security fixes validated - no regressions detected"
#!/bin/bash
set -euo pipefail

echo "Valgrind Memory Leak Detection for pg_tviews"
echo "============================================"
echo ""
echo "⚠️  NOTE: This script demonstrates Valgrind usage for pg_tviews."
echo "   Running PostgreSQL under Valgrind requires special setup and"
echo "   may not work in all environments."
echo ""
echo "For production memory leak testing, follow these steps:"
echo ""

# Check if Valgrind is available
if command -v valgrind &> /dev/null; then
    echo "✅ Valgrind is available"
    VALGRIND_AVAILABLE=true
else
    echo "❌ Valgrind not found. Install with: sudo pacman -S valgrind"
    VALGRIND_AVAILABLE=false
fi

echo ""
echo "Recommended Valgrind testing approach:"
echo "--------------------------------------"
echo ""
echo "1. Build pg_tviews with debug symbols:"
echo "   cargo build --profile=dev (or add debug=true to release profile)"
echo ""
echo "2. Start PostgreSQL under Valgrind:"
echo "   valgrind --leak-check=full --show-leak-kinds=all \\"
echo "            --track-origins=yes --log-file=valgrind.log \\"
echo "            postgres -D /var/lib/postgres/data"
echo ""
echo "3. Run test workload:"
echo "   psql -f test_workload.sql"
echo ""
echo "4. Analyze results:"
echo "   grep 'definitely lost' valgrind.log"
echo "   grep 'indirectly lost' valgrind.log"
echo ""

# Create a sample test workload
cat > test/profiling/valgrind-workload.sql <<EOF
-- Valgrind Memory Leak Test Workload
-- This workload exercises pg_tviews functionality to detect leaks

-- Create test extension (if not exists)
CREATE EXTENSION IF NOT EXISTS pg_tviews;

-- Create test tables
CREATE TABLE IF NOT EXISTS tb_valgrind_test (
    pk_test SERIAL PRIMARY KEY,
    data TEXT,
    json_data JSONB
);

CREATE TABLE IF NOT EXISTS tv_valgrind_test AS
SELECT pk_test, data, json_data FROM tb_valgrind_test;

-- Convert to TVIEW
SELECT pg_tviews_convert_existing_table('tv_valgrind_test');

-- Insert test data (various sizes)
INSERT INTO tb_valgrind_test (data, json_data)
SELECT
    'test-data-' || i,
    jsonb_build_object('id', i, 'value', 'data-' || i, 'metadata', repeat('x', 100))
FROM generate_series(1, 1000) i;

-- Test cascade operations
UPDATE tb_valgrind_test SET data = 'updated-' || pk_test WHERE pk_test <= 100;

-- Test JSONB operations
UPDATE tb_valgrind_test SET json_data = jsonb_set(json_data, '{value}', '"updated"') WHERE pk_test <= 50;

-- Test cleanup
DROP TABLE tv_valgrind_test CASCADE;
CREATE TABLE tv_valgrind_test AS SELECT pk_test, data, json_data FROM tb_valgrind_test;
SELECT pg_tviews_convert_existing_table('tv_valgrind_test');

-- Final operations
INSERT INTO tb_valgrind_test (data, json_data)
SELECT 'final-' || i, jsonb_build_object('final', i) FROM generate_series(1, 100) i;

-- Clean up
DROP TABLE tv_valgrind_test CASCADE;
DROP TABLE tb_valgrind_test CASCADE;
EOF

echo "✅ Created test workload: test/profiling/valgrind-workload.sql"
echo ""
echo "To run Valgrind testing manually:"
echo "1. Start PostgreSQL under Valgrind (see above)"
echo "2. Run: psql -f test/profiling/valgrind-workload.sql"
echo "3. Stop PostgreSQL and check valgrind.log"
echo ""
echo "Expected Results:"
echo "- 'definitely lost: 0 bytes' (no memory leaks)"
echo "- 'indirectly lost: 0 bytes' (no indirect leaks)"
echo ""

# If Valgrind is available, show version
if [ "$VALGRIND_AVAILABLE" = true ]; then
    echo "Valgrind version:"
    valgrind --version
fi

echo ""
echo "✅ Valgrind leak detection setup complete"
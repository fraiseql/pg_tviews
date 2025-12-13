#!/bin/bash
set -euo pipefail

echo "Measuring baseline memory usage..."

# Function to get PostgreSQL memory usage
get_pg_memory() {
    local pid=$(pidof postgres 2>/dev/null | head -1)
    if [ -z "$pid" ]; then
        echo "ERROR"
        return 1
    fi

    # Get RSS (Resident Set Size) in KB
    local rss_kb=$(ps -p "$pid" -o rss= 2>/dev/null | awk '{print $1}' | head -1)
    echo "$rss_kb"
}

# Function to get heap size from /proc
get_heap_size() {
    local pid=$(pidof postgres 2>/dev/null | head -1)
    if [ -z "$pid" ]; then
        echo "ERROR"
        return 1
    fi

    # Get heap size in KB from /proc/[pid]/status
    local heap_kb=$(grep "^VmData:" "/proc/$pid/status" 2>/dev/null | awk '{print $2}' | head -1)
    echo "$heap_kb"
}

echo "Starting PostgreSQL..."
# Note: This assumes PostgreSQL is already configured and can be started
# In a real environment, you'd use systemctl or pg_ctl

# Wait for PostgreSQL to be ready
sleep 5

# Get baseline memory
BASELINE_RSS=$(get_pg_memory)
BASELINE_HEAP=$(get_heap_size)

if [ "$BASELINE_RSS" = "ERROR" ]; then
    echo "❌ PostgreSQL not running. Please start PostgreSQL first."
    exit 1
fi

echo "Baseline PostgreSQL memory: ${BASELINE_RSS}KB RSS, ${BASELINE_HEAP}KB heap"

# Load extension
echo "Loading pg_tviews extension..."
psql -c "CREATE EXTENSION IF NOT EXISTS pg_tviews;" 2>/dev/null || {
    echo "⚠️  Extension not available, continuing with baseline measurement"
}

# After extension load
AFTER_EXT_RSS=$(get_pg_memory)
AFTER_EXT_HEAP=$(get_heap_size)
echo "After extension load: ${AFTER_EXT_RSS}KB RSS, ${AFTER_EXT_HEAP}KB heap"

# Create test TVIEW
echo "Creating test TVIEW..."
psql <<EOF 2>/dev/null || echo "⚠️  Database operations failed, continuing..."
CREATE TABLE IF NOT EXISTS tb_baseline (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE IF NOT EXISTS tv_baseline AS SELECT pk_test, data FROM tb_baseline;
SELECT pg_tviews_convert_existing_table('tv_baseline');
EOF

AFTER_TVIEW_RSS=$(get_pg_memory)
AFTER_TVIEW_HEAP=$(get_heap_size)
echo "After TVIEW creation: ${AFTER_TVIEW_RSS}KB RSS, ${AFTER_TVIEW_HEAP}KB heap"

# Insert 10K rows
echo "Inserting 10K rows..."
psql <<EOF 2>/dev/null || echo "⚠️  Insert failed, continuing..."
INSERT INTO tb_baseline (data) SELECT 'row-' || i FROM generate_series(1, 10000) i;
EOF

AFTER_REFRESH_RSS=$(get_pg_memory)
AFTER_REFRESH_HEAP=$(get_heap_size)
echo "After 10K row refresh: ${AFTER_REFRESH_RSS}KB RSS, ${AFTER_REFRESH_HEAP}KB heap"

# Calculate deltas
EXT_DELTA_RSS=$((AFTER_EXT_RSS - BASELINE_RSS))
EXT_DELTA_HEAP=$((AFTER_EXT_HEAP - BASELINE_HEAP))

TVIEW_DELTA_RSS=$((AFTER_TVIEW_RSS - AFTER_EXT_RSS))
TVIEW_DELTA_HEAP=$((AFTER_TVIEW_HEAP - AFTER_EXT_HEAP))

REFRESH_DELTA_RSS=$((AFTER_REFRESH_RSS - AFTER_TVIEW_RSS))
REFRESH_DELTA_HEAP=$((AFTER_REFRESH_HEAP - AFTER_TVIEW_HEAP))

# Save baseline
cat > test/profiling/baseline-memory.txt <<EOF
=== pg_tviews Memory Baseline ===

Baseline PostgreSQL: ${BASELINE_RSS}KB RSS, ${BASELINE_HEAP}KB heap
After extension: ${AFTER_EXT_RSS}KB RSS (+${EXT_DELTA_RSS}KB), ${AFTER_EXT_HEAP}KB heap (+${EXT_DELTA_HEAP}KB)
After TVIEW: ${AFTER_TVIEW_RSS}KB RSS (+${TVIEW_DELTA_RSS}KB), ${AFTER_TVIEW_HEAP}KB heap (+${TVIEW_DELTA_HEAP}KB)
After 10K refresh: ${AFTER_REFRESH_RSS}KB RSS (+${REFRESH_DELTA_RSS}KB), ${AFTER_REFRESH_HEAP}KB heap (+${REFRESH_DELTA_HEAP}KB)

=== Memory Budget Assessment ===
Extension load: ${EXT_DELTA_RSS}KB RSS (target: <10MB)
TVIEW creation: ${TVIEW_DELTA_RSS}KB RSS (target: <5MB)
10K row refresh: ${REFRESH_DELTA_RSS}KB RSS (target: <50MB)

EOF

echo "✅ Baseline saved to test/profiling/baseline-memory.txt"
cat test/profiling/baseline-memory.txt
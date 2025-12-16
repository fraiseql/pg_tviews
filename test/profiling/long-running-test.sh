#!/bin/bash
set -euo pipefail

echo "Memory Stability Test"
echo "====================="
echo ""

# Configuration
DURATION_MINUTES=${1:-60}  # Default 1 hour, configurable
INTERVAL_SECONDS=30        # Check every 30 seconds

echo "Running memory stability test for ${DURATION_MINUTES} minutes..."
echo "Checking memory every ${INTERVAL_SECONDS} seconds"
echo ""

# Function to get PostgreSQL memory
get_pg_memory() {
    local pid=$(pidof postgres | head -1 2>/dev/null)
    if [ -z "$pid" ]; then
        echo "ERROR"
        return 1
    fi

    # Get RSS in KB
    local rss=$(ps -p "$pid" -o rss= 2>/dev/null | awk '{print $1}')
    echo "$rss"
}

# Function to get heap size
get_heap_size() {
    local pid=$(pidof postgres | head -1 2>/dev/null)
    if [ -z "$pid" ]; then
        echo "ERROR"
        return 1
    fi

    # Get heap from /proc
    local heap=$(grep "^VmData:" "/proc/$pid/status" 2>/dev/null | awk '{print $2}')
    echo "$heap"
}

# Check if PostgreSQL is running
INITIAL_MEMORY=$(get_pg_memory)
if [ "$INITIAL_MEMORY" = "ERROR" ]; then
    echo "❌ PostgreSQL not running. Please start PostgreSQL first."
    exit 1
fi

echo "Initial PostgreSQL memory: ${INITIAL_MEMORY}KB RSS"

# Create CSV file for results
OUTPUT_FILE="test/profiling/memory-stability-$(date +%Y%m%d_%H%M%S).csv"
echo "timestamp,rss_kb,heap_kb,operation" > "$OUTPUT_FILE"

# Calculate number of iterations
TOTAL_SECONDS=$((DURATION_MINUTES * 60))
ITERATIONS=$((TOTAL_SECONDS / INTERVAL_SECONDS))

echo "Starting test with $ITERATIONS measurements..."
echo ""

START_TIME=$(date +%s)

for i in $(seq 1 "$ITERATIONS"); do
    CURRENT_TIME=$(date +%s)
    ELAPSED=$((CURRENT_TIME - START_TIME))
    PERCENT=$((i * 100 / ITERATIONS))

    # Get current memory
    RSS=$(get_pg_memory)
    HEAP=$(get_heap_size)

    if [ "$RSS" = "ERROR" ] || [ "$HEAP" = "ERROR" ]; then
        echo "❌ PostgreSQL stopped running"
        break
    fi

    # Perform some operations to stress memory
    case $((i % 4)) in
        0)
            # Insert operation
            psql -c "CREATE TABLE IF NOT EXISTS tb_mem_test (id SERIAL PRIMARY KEY, data TEXT);" 2>/dev/null
            psql -c "INSERT INTO tb_mem_test (data) SELECT 'test-' || i FROM generate_series(1, 10);" 2>/dev/null
            OPERATION="insert"
            ;;
        1)
            # Update operation
            psql -c "UPDATE tb_mem_test SET data = 'updated-' || id WHERE id <= 5;" 2>/dev/null
            OPERATION="update"
            ;;
        2)
            # TVIEW refresh
            psql -c "CREATE TABLE IF NOT EXISTS tv_mem_test AS SELECT id, data FROM tb_mem_test;" 2>/dev/null
            psql -c "SELECT pg_tviews_convert_existing_table('tv_mem_test');" 2>/dev/null
            OPERATION="tview_refresh"
            ;;
        3)
            # Cleanup
            psql -c "DROP TABLE IF EXISTS tv_mem_test CASCADE;" 2>/dev/null
            OPERATION="cleanup"
            ;;
    esac

    # Record data
    echo "$CURRENT_TIME,$RSS,$HEAP,$OPERATION" >> "$OUTPUT_FILE"

    # Progress indicator
    if [ $((i % 10)) -eq 0 ]; then
        echo "[$i/$ITERATIONS] ${PERCENT}% complete - RSS: ${RSS}KB, Heap: ${HEAP}KB"
    fi

    # Wait for next interval
    sleep "$INTERVAL_SECONDS"
done

END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

echo ""
echo "✅ Memory stability test completed"
echo "Duration: ${DURATION} seconds"
echo "Measurements: $i"
echo "Results saved to: $OUTPUT_FILE"

# Basic analysis
echo ""
echo "Basic Analysis:"
echo "==============="

# Calculate memory range
MIN_RSS=$(tail -n +2 "$OUTPUT_FILE" | cut -d',' -f2 | sort -n | head -1)
MAX_RSS=$(tail -n +2 "$OUTPUT_FILE" | cut -d',' -f2 | sort -n | tail -1)
RANGE_RSS=$((MAX_RSS - MIN_RSS))

echo "RSS Memory Range: ${MIN_RSS}KB - ${MAX_RSS}KB (${RANGE_RSS}KB variation)"

# Check for concerning trends
if [ "$RANGE_RSS" -gt 50000 ]; then  # More than 50MB variation
    echo "⚠️  WARNING: Large memory variation detected (${RANGE_RSS}KB)"
    echo "   This may indicate a memory leak or unstable memory usage"
else
    echo "✅ Memory usage appears stable"
fi

echo ""
echo "For detailed analysis, examine: $OUTPUT_FILE"
echo "You can import this CSV into spreadsheet software for charting."
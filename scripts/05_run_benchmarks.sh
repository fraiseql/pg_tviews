#!/bin/bash
set -euo pipefail

echo "=== RUNNING BENCHMARKS ==="

# Configuration
CONTAINER_NAME="pg_tviews_benchmark"
SCALES="${BENCHMARK_SCALES:-small medium large}"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_LOG="/tmp/benchmark_results_${TIMESTAMP}.log"
STATS_LOG="/tmp/benchmark_stats_${TIMESTAMP}.log"
METADATA_FILE="/tmp/benchmark_metadata_${TIMESTAMP}.json"

echo "Configuration:"
echo "  Scales: $SCALES"
echo "  Results log: $RESULTS_LOG"
echo "  Stats log: $STATS_LOG"
echo "  Metadata: $METADATA_FILE"
echo ""

# Verify container is running
if ! podman ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    echo "ERROR: Container '$CONTAINER_NAME' is not running"
    echo "Run smoke test first: ./04_smoke_test.sh"
    exit 1
fi

# Verify PostgreSQL is ready
if ! podman exec "$CONTAINER_NAME" pg_isready -U postgres > /dev/null 2>&1; then
    echo "ERROR: PostgreSQL is not ready"
    exit 1
fi

echo "✓ Container and PostgreSQL ready"
echo ""

# Generate metadata
BUILD_METADATA_PATH=$(cat /tmp/build_metadata_path.txt 2>/dev/null || echo "")
if [[ -f "$BUILD_METADATA_PATH" ]]; then
    BUILD_METADATA=$(cat "$BUILD_METADATA_PATH")
else
    BUILD_METADATA="{}"
fi

cat > "$METADATA_FILE" <<EOF
{
  "benchmark_run": {
    "timestamp": "$(date -Iseconds)",
    "scales": $(echo "$SCALES" | jq -R 'split(" ")'),
    "git_sha": "$(git rev-parse HEAD)",
    "git_branch": "$(git branch --show-current)",
    "git_dirty": $(git diff-index --quiet HEAD -- && echo "false" || echo "true")
  },
  "environment": {
    "podman_version": "$(podman --version | awk '{print $3}')",
    "kernel_version": "$(uname -r)",
    "hostname": "$(hostname)",
    "cpu_count": $(nproc),
    "total_memory_gb": $(free -g | awk 'NR==2 {print $2}')
  },
  "container": {
    "name": "$CONTAINER_NAME",
    "memory_limit": $(podman inspect "$CONTAINER_NAME" --format '{{.HostConfig.Memory}}'),
    "cpu_quota": $(podman inspect "$CONTAINER_NAME" --format '{{.HostConfig.CpuQuota}}')
  },
  "build": $BUILD_METADATA
}
EOF

echo "✓ Metadata generated: $METADATA_FILE"
echo ""

# Start resource monitoring in background
echo "Starting resource monitoring..."
{
    while true; do
        podman stats "$CONTAINER_NAME" --no-stream --format "{{.MemUsage}}\t{{.CPUPerc}}\t{{.NetIO}}\t{{.BlockIO}}"
        sleep 5
    done
} > "$STATS_LOG" 2>&1 &
MONITOR_PID=$!

echo "✓ Monitor started (PID: $MONITOR_PID)"
echo ""

# Trap to cleanup monitor on exit
trap "kill $MONITOR_PID 2>/dev/null || true" EXIT INT TERM

# Run benchmarks
echo "=========================================="
echo "STARTING BENCHMARK EXECUTION"
echo "=========================================="
echo ""

START_TIME=$(date +%s)

if ! podman exec "$CONTAINER_NAME" bash -c "
    set -euo pipefail
    export PGHOST=localhost
    export PGUSER=postgres
    export PGDATABASE=postgres

    cd /benchmarks
    ./run_benchmarks.sh --scale \"$SCALES\" 2>&1
" | tee "$RESULTS_LOG"; then
    echo ""
    echo "ERROR: Benchmark execution failed"
    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))

    # Record failure in metadata
    jq ".benchmark_run.status = \"failed\" | .benchmark_run.duration_seconds = $DURATION" \
        "$METADATA_FILE" > "${METADATA_FILE}.tmp" && mv "${METADATA_FILE}.tmp" "$METADATA_FILE"

    exit 1
fi

END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

echo ""
echo "=========================================="
echo "BENCHMARK EXECUTION COMPLETE"
echo "=========================================="
echo ""
echo "Duration: ${DURATION}s ($(($DURATION / 60))m $(($DURATION % 60))s)"
echo ""

# Stop monitoring
kill $MONITOR_PID 2>/dev/null || true
trap - EXIT INT TERM

echo "✓ Monitoring stopped"
echo ""

# Update metadata with results
jq ".benchmark_run.status = \"success\" | .benchmark_run.duration_seconds = $DURATION" \
    "$METADATA_FILE" > "${METADATA_FILE}.tmp" && mv "${METADATA_FILE}.tmp" "$METADATA_FILE"

# Generate summary statistics
echo "=== RESOURCE USAGE SUMMARY ==="
echo ""
echo "Peak memory usage:"
sort -k1 -h "$STATS_LOG" | tail -1 | awk '{print "  " $1}'
echo ""
echo "Average CPU usage:"
awk '{sum+=$2; count++} END {if(count>0) print "  " sum/count "%"}' "$STATS_LOG"
echo ""

echo "=== RESULTS ==="
echo ""
echo "Results log: $RESULTS_LOG"
echo "Stats log: $STATS_LOG"
echo "Metadata: $METADATA_FILE"
echo ""

# Save artifact paths for collection
cat > /tmp/artifact_paths.txt <<EOF
$RESULTS_LOG
$STATS_LOG
$METADATA_FILE
EOF

echo "✓ Artifact paths saved"
echo ""

echo "=== BENCHMARKS COMPLETE ==="

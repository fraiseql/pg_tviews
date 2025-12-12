#!/bin/bash
set -euo pipefail

echo "=== SMOKE TEST ==="

# Configuration
IMAGE_NAME="localhost/pg_tviews_bench:latest"
CONTAINER_NAME="pg_tviews_benchmark"
STARTUP_TIMEOUT=60

echo "Starting container for smoke test..."

# Run container with resource limits
podman run -d \
    --name "$CONTAINER_NAME" \
    --memory=4g \
    --memory-swap=4g \
    --cpus=2 \
    --shm-size=512m \
    "$IMAGE_NAME"

echo "✓ Container started"
echo ""

# Verify resource limits were applied
echo "Resource limits:"
MEMORY_LIMIT=$(podman inspect "$CONTAINER_NAME" --format '{{.HostConfig.Memory}}')
CPU_QUOTA=$(podman inspect "$CONTAINER_NAME" --format '{{.HostConfig.CpuQuota}}')
echo "  Memory: $((MEMORY_LIMIT / 1024 / 1024 / 1024))GB"
echo "  CPU quota: $CPU_QUOTA"
echo ""

# Wait for PostgreSQL to be ready (with timeout)
echo "Waiting for PostgreSQL to start (timeout: ${STARTUP_TIMEOUT}s)..."

if ! timeout $STARTUP_TIMEOUT bash -c "
    while ! podman exec $CONTAINER_NAME pg_isready -U postgres 2>/dev/null; do
        echo '  Waiting for PostgreSQL...'
        sleep 2
    done
"; then
    echo ""
    echo "ERROR: PostgreSQL failed to start within ${STARTUP_TIMEOUT}s"
    echo "Container logs:"
    podman logs "$CONTAINER_NAME" | tail -50

    # Cleanup failed container
    podman stop "$CONTAINER_NAME" 2>/dev/null || true
    podman rm "$CONTAINER_NAME" 2>/dev/null || true
    exit 1
fi

echo "✓ PostgreSQL is ready"
echo ""

# Test 1: Database listing
echo "Test 1: Database connectivity..."
if ! podman exec "$CONTAINER_NAME" psql -U postgres -c '\l' > /dev/null 2>&1; then
    echo "ERROR: Cannot list databases"
    podman logs "$CONTAINER_NAME" | tail -50
    exit 1
fi
echo "✓ Database connectivity OK"

# Test 2: Extension check
echo "Test 2: Extension availability..."
if ! podman exec "$CONTAINER_NAME" psql -U postgres -d pg_tviews_benchmark -c '\dx' 2>&1 | grep -q pg_tviews; then
    echo "ERROR: pg_tviews extension not found"
    echo "Available extensions:"
    podman exec "$CONTAINER_NAME" psql -U postgres -d pg_tviews_benchmark -c '\dx'
    exit 1
fi
echo "✓ pg_tviews extension found"

# Test 3: Version check
echo "Test 3: PostgreSQL version..."
PG_VERSION=$(podman exec "$CONTAINER_NAME" psql -U postgres -t -c 'SELECT version();' | xargs)
echo "  $PG_VERSION"
echo "✓ Version check OK"

# Test 4: Benchmark scripts present
echo "Test 4: Benchmark scripts..."
if ! podman exec "$CONTAINER_NAME" test -f /benchmarks/run_benchmarks.sh; then
    echo "ERROR: Benchmark script not found"
    exit 1
fi
if ! podman exec "$CONTAINER_NAME" test -x /benchmarks/run_benchmarks.sh; then
    echo "ERROR: Benchmark script not executable"
    exit 1
fi
echo "✓ Benchmark scripts present and executable"

# Test 5: Quick sanity query
echo "Test 5: Query execution..."
if ! podman exec "$CONTAINER_NAME" psql -U postgres -c "SELECT 1;" > /dev/null 2>&1; then
    echo "ERROR: Cannot execute queries"
    exit 1
fi
echo "✓ Query execution OK"

# Test 6: Container resource usage (baseline)
echo "Test 6: Resource baseline..."
podman stats "$CONTAINER_NAME" --no-stream --format "table {{.Name}}\t{{.MemUsage}}\t{{.CPUPerc}}" | tail -1

echo ""
echo "=== ALL SMOKE TESTS PASSED ==="
echo ""
echo "Container '$CONTAINER_NAME' is ready for benchmarks"
echo ""

#!/bin/bash
# Helper script to run benchmarks against Docker PostgreSQL

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}=== pg_tviews Benchmark Runner (Docker)${NC}"
echo

# Check if Docker container is running
if ! docker ps | grep -q pg_tviews_bench; then
    echo -e "${RED}Error: Docker container 'pg_tviews_bench' is not running${NC}"
    echo "Start it with: cd docker && docker compose up -d"
    exit 1
fi

echo -e "${GREEN}✓ Docker container running${NC}"

# Wait for PostgreSQL to be ready
echo -n "Waiting for PostgreSQL to be ready..."
for i in {1..30}; do
    if docker exec pg_tviews_bench pg_isready -U postgres -d pg_tviews_benchmark > /dev/null 2>&1; then
        echo -e " ${GREEN}✓${NC}"
        break
    fi
    echo -n "."
    sleep 1
    if [ $i -eq 30 ]; then
        echo -e " ${RED}✗${NC}"
        echo -e "${RED}Error: PostgreSQL not ready after 30 seconds${NC}"
        echo "Check logs with: docker logs pg_tviews_bench"
        exit 1
    fi
done

# Run benchmarks inside the container
echo -e "${YELLOW}Running benchmarks...${NC}"
echo

# Default to small scale if not specified
SCALE=${1:-small}

docker exec -it pg_tviews_bench bash -c "
    cd /benchmarks
    ./run_benchmarks.sh --scale $SCALE
"

EXIT_CODE=$?

if [ $EXIT_CODE -eq 0 ]; then
    echo
    echo -e "${GREEN}✓ Benchmarks completed successfully${NC}"
    echo
    echo "Results are in: test/sql/comprehensive_benchmarks/results/"
    ls -lth test/sql/comprehensive_benchmarks/results/ | head -5
else
    echo
    echo -e "${RED}✗ Benchmarks failed with exit code $EXIT_CODE${NC}"
    echo "Check Docker logs: docker logs pg_tviews_bench"
    exit $EXIT_CODE
fi

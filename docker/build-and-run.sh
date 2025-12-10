#!/bin/bash
# Simple Docker build and run script (no docker-compose required)

set -e

CONTAINER_NAME="pg_tviews_bench"
IMAGE_NAME="pg_tviews-benchmarks"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

usage() {
    echo "pg_tviews Docker Benchmark Script (Simple Version)"
    echo ""
    echo "Usage: $0 <command> [options]"
    echo ""
    echo "Commands:"
    echo "  build              Build the Docker image"
    echo "  start              Start container"
    echo "  stop               Stop container"
    echo "  remove             Remove container"
    echo "  run <scale>        Run benchmarks (small|medium|large)"
    echo "  shell              Open shell in container"
    echo "  psql               Connect to PostgreSQL"
    echo "  logs               Show container logs"
    echo ""
}

case "${1:-help}" in
    build)
        echo -e "${BLUE}Building Docker image...${NC}"
        docker build -t "$IMAGE_NAME" -f Dockerfile.benchmarks .
        echo -e "${GREEN}✓ Image built successfully${NC}"
        ;;

    start)
        # Remove existing container if present
        docker rm -f "$CONTAINER_NAME" 2>/dev/null || true

        echo -e "${BLUE}Starting container...${NC}"
        docker run -d \
            --name "$CONTAINER_NAME" \
            -e POSTGRES_DB=pg_tviews_benchmark \
            -e POSTGRES_USER=postgres \
            -e POSTGRES_PASSWORD=postgres \
            -p 5433:5432 \
            --shm-size=1g \
            -v "$(pwd)/test/sql/comprehensive_benchmarks/results:/benchmarks/results" \
            "$IMAGE_NAME"

        echo -e "${YELLOW}Waiting for PostgreSQL to be ready...${NC}"
        sleep 5
        timeout 60 bash -c "until docker exec $CONTAINER_NAME pg_isready -U postgres > /dev/null 2>&1; do sleep 2; done" || {
            echo -e "${RED}✗ PostgreSQL failed to start${NC}"
            docker logs "$CONTAINER_NAME"
            exit 1
        }

        echo -e "${GREEN}✓ Container started successfully${NC}"
        echo ""
        echo "Extensions installed:"
        docker exec "$CONTAINER_NAME" psql -U postgres -d pg_tviews_benchmark -c "\dx"
        ;;

    stop)
        echo -e "${BLUE}Stopping container...${NC}"
        docker stop "$CONTAINER_NAME"
        echo -e "${GREEN}✓ Container stopped${NC}"
        ;;

    remove)
        echo -e "${BLUE}Removing container...${NC}"
        docker rm -f "$CONTAINER_NAME"
        echo -e "${GREEN}✓ Container removed${NC}"
        ;;

    run)
        SCALE="${2:-small}"
        if [[ ! "$SCALE" =~ ^(small|medium|large)$ ]]; then
            echo -e "${RED}✗ Invalid scale: $SCALE${NC}"
            echo "Valid scales: small, medium, large"
            exit 1
        fi

        echo -e "${BLUE}Running ${SCALE} scale benchmarks...${NC}"
        docker exec -it "$CONTAINER_NAME" /benchmarks/run_benchmarks.sh --scale "$SCALE"
        echo ""
        echo -e "${GREEN}✓ Benchmarks complete${NC}"
        echo "Results saved to: test/sql/comprehensive_benchmarks/results/"
        ;;

    shell)
        echo -e "${BLUE}Opening shell...${NC}"
        docker exec -it "$CONTAINER_NAME" bash
        ;;

    psql)
        echo -e "${BLUE}Connecting to PostgreSQL...${NC}"
        docker exec -it "$CONTAINER_NAME" psql -U postgres -d pg_tviews_benchmark
        ;;

    logs)
        docker logs -f "$CONTAINER_NAME"
        ;;

    help|--help|-h)
        usage
        ;;

    *)
        echo -e "${RED}Unknown command: $1${NC}"
        usage
        exit 1
        ;;
esac

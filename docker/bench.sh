#!/bin/bash
# Helper script for Docker-based benchmarking

set -e

CONTAINER_NAME="pg_tviews_bench"
SERVICE_NAME="pg_tviews_bench"
IMAGE_NAME="pg_tviews-benchmarks"

# Detect docker compose command
if command -v docker-compose >/dev/null 2>&1; then
    DOCKER_COMPOSE="docker-compose"
elif docker compose version >/dev/null 2>&1; then
    DOCKER_COMPOSE="docker compose"
else
    echo -e "${RED}✗ Docker Compose not found${NC}"
    echo "Please install Docker Compose: https://docs.docker.com/compose/install/"
    exit 1
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

usage() {
    echo "pg_tviews Docker Benchmark Helper"
    echo ""
    echo "Usage: $0 <command> [options]"
    echo ""
    echo "Commands:"
    echo "  build              Build the benchmark container"
    echo "  start              Start the container"
    echo "  stop               Stop the container"
    echo "  restart            Restart the container"
    echo "  status             Show container status"
    echo "  logs               Show container logs"
    echo "  run <scale>        Run benchmarks (scale: small|medium|large)"
    echo "  report             Generate markdown report"
    echo "  psql               Connect to PostgreSQL"
    echo "  shell              Open bash shell in container"
    echo "  results            Show latest benchmark results"
    echo "  clean              Stop and remove container (keep volumes)"
    echo "  purge              Remove everything (container, volumes, images)"
    echo ""
    echo "Examples:"
    echo "  $0 build                    # Build container"
    echo "  $0 start                    # Start container"
    echo "  $0 run small                # Run small-scale benchmarks"
    echo "  $0 run medium               # Run medium-scale benchmarks"
    echo "  $0 results                  # View latest results"
    echo "  $0 psql                     # Connect to database"
    echo ""
}

check_container() {
    if ! docker ps -q -f name="$CONTAINER_NAME" > /dev/null 2>&1; then
        return 1
    fi
    return 0
}

check_container_running() {
    if ! docker ps -q -f name="$CONTAINER_NAME" -f status=running > /dev/null 2>&1; then
        echo -e "${RED}✗ Container is not running${NC}"
        echo "Start it with: $0 start"
        return 1
    fi
    return 0
}

case "${1:-help}" in
    build)
        echo -e "${BLUE}Building benchmark container...${NC}"
        $DOCKER_COMPOSE build $SERVICE_NAME
        echo -e "${GREEN}✓ Build complete${NC}"
        ;;

    start)
        echo -e "${BLUE}Starting benchmark container...${NC}"
        $DOCKER_COMPOSE up -d $SERVICE_NAME
        echo -e "${YELLOW}Waiting for PostgreSQL to be ready...${NC}"
        sleep 5
        timeout 60 bash -c "until docker exec $CONTAINER_NAME pg_isready -U postgres > /dev/null 2>&1; do sleep 2; done" || {
            echo -e "${RED}✗ PostgreSQL failed to start${NC}"
            exit 1
        }
        echo -e "${GREEN}✓ Container started and ready${NC}"
        docker exec -it $CONTAINER_NAME psql -U postgres -d pg_tviews_benchmark -c "\dx" 2>/dev/null || true
        ;;

    stop)
        echo -e "${BLUE}Stopping benchmark container...${NC}"
        $DOCKER_COMPOSE stop $SERVICE_NAME
        echo -e "${GREEN}✓ Container stopped${NC}"
        ;;

    restart)
        echo -e "${BLUE}Restarting benchmark container...${NC}"
        $DOCKER_COMPOSE restart $SERVICE_NAME
        sleep 5
        echo -e "${GREEN}✓ Container restarted${NC}"
        ;;

    status)
        echo -e "${BLUE}Container Status:${NC}"
        $DOCKER_COMPOSE ps $SERVICE_NAME
        echo ""
        if check_container_running; then
            echo -e "${BLUE}PostgreSQL Status:${NC}"
            docker exec $CONTAINER_NAME pg_isready -U postgres
            echo ""
            echo -e "${BLUE}Installed Extensions:${NC}"
            docker exec $CONTAINER_NAME psql -U postgres -d pg_tviews_benchmark -c "\dx"
        fi
        ;;

    logs)
        $DOCKER_COMPOSE logs -f $SERVICE_NAME
        ;;

    run)
        SCALE="${2:-small}"
        if [[ ! "$SCALE" =~ ^(small|medium|large)$ ]]; then
            echo -e "${RED}✗ Invalid scale: $SCALE${NC}"
            echo "Valid scales: small, medium, large"
            exit 1
        fi

        if ! check_container_running; then
            exit 1
        fi

        echo -e "${BLUE}Running ${SCALE} scale benchmarks...${NC}"
        docker exec -it $CONTAINER_NAME /benchmarks/run_benchmarks.sh --scale "$SCALE"
        echo ""
        echo -e "${GREEN}✓ Benchmarks complete${NC}"
        echo -e "${YELLOW}View results with: $0 results${NC}"
        echo -e "${YELLOW}Generate report with: $0 report${NC}"
        ;;

    report)
        if ! check_container_running; then
            exit 1
        fi

        echo -e "${BLUE}Generating markdown report...${NC}"
        docker exec -it $CONTAINER_NAME python3 /benchmarks/generate_report.py
        echo ""
        echo -e "${GREEN}✓ Report generated${NC}"
        echo "Reports available in: test/sql/comprehensive_benchmarks/results/"
        ;;

    psql)
        if ! check_container_running; then
            exit 1
        fi

        echo -e "${BLUE}Connecting to PostgreSQL...${NC}"
        docker exec -it $CONTAINER_NAME psql -U postgres -d pg_tviews_benchmark
        ;;

    shell)
        if ! check_container_running; then
            exit 1
        fi

        echo -e "${BLUE}Opening shell in container...${NC}"
        docker exec -it $CONTAINER_NAME bash
        ;;

    results)
        if ! check_container_running; then
            exit 1
        fi

        echo -e "${BLUE}Latest benchmark results:${NC}"
        LATEST_LOG=$(docker exec $CONTAINER_NAME ls -t /benchmarks/results/benchmark_run_*.log 2>/dev/null | head -1)
        if [ -n "$LATEST_LOG" ]; then
            docker exec $CONTAINER_NAME tail -100 "$LATEST_LOG"
        else
            echo -e "${YELLOW}No benchmark results found${NC}"
            echo "Run benchmarks with: $0 run small"
        fi
        ;;

    clean)
        echo -e "${BLUE}Cleaning up (keeping volumes)...${NC}"
        $DOCKER_COMPOSE down
        echo -e "${GREEN}✓ Container removed (data preserved)${NC}"
        ;;

    purge)
        echo -e "${RED}WARNING: This will delete all benchmark data and images${NC}"
        read -p "Are you sure? (yes/no): " confirm
        if [ "$confirm" = "yes" ]; then
            echo -e "${BLUE}Purging everything...${NC}"
            $DOCKER_COMPOSE down -v
            docker rmi pg_tviews-pg_tviews_bench 2>/dev/null || true
            rm -rf test/sql/comprehensive_benchmarks/results/*
            echo -e "${GREEN}✓ Complete cleanup finished${NC}"
        else
            echo "Cancelled"
        fi
        ;;

    help|--help|-h)
        usage
        ;;

    *)
        echo -e "${RED}Unknown command: $1${NC}"
        echo ""
        usage
        exit 1
        ;;
esac

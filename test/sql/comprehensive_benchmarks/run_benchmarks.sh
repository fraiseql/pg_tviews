#!/bin/bash
# Comprehensive Benchmark Runner for pg_tviews
# Runs all benchmark scenarios across multiple data scales

set -e

# Configuration
DB_NAME="pg_tviews_benchmark"
PSQL="psql -d $DB_NAME -v ON_ERROR_STOP=1"
RESULTS_DIR="results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOG_FILE="$RESULTS_DIR/benchmark_run_$TIMESTAMP.log"

# Cleanup on error/exit
cleanup_on_exit() {
    local exit_code=$?
    if [ $exit_code -ne 0 ]; then
        echo -e "${YELLOW}Benchmark failed, cleaning up partial state...${NC}"

        # Drop benchmark schema to clean up partial state
        psql -d "$DB_NAME" -c "DROP SCHEMA IF EXISTS benchmark CASCADE;" 2>/dev/null || true

        echo -e "${GREEN}✓ Cleanup complete${NC}"
    fi
}

trap cleanup_on_exit EXIT INT TERM

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Create results directory
mkdir -p "$RESULTS_DIR" 2>/dev/null || RESULTS_DIR="/tmp"

echo -e "${BLUE}=========================================${NC}"
echo -e "${BLUE}pg_tviews Comprehensive Benchmark Suite${NC}"
echo -e "${BLUE}=========================================${NC}"
echo ""
echo "Started at: $(date)"
echo "Results will be saved to: $LOG_FILE"
echo ""

# Log function
log() {
    echo -e "$1"
}

# Error handler
error_exit() {
    log "${RED}Error: $1${NC}"
    exit 1
}

# Diagnostic function to check database state
check_database_state() {
    log "  Database state:"

    local table_count=$($PSQL -t -c "SELECT COUNT(*) FROM pg_tables WHERE schemaname = 'benchmark' AND (tablename LIKE 'tb_%' OR tablename LIKE 'pk_%');")
    local view_count=$($PSQL -t -c "SELECT COUNT(*) FROM pg_matviews WHERE schemaname = 'benchmark';")
    local trigger_count=$($PSQL -t -c "SELECT COUNT(*) FROM pg_event_trigger WHERE evtname LIKE 'pg_tviews%';")

    log "    Tables: ${table_count}, Views: ${view_count}, Triggers: ${trigger_count}"
}

# Check PostgreSQL connection
log "${YELLOW}Checking PostgreSQL connection...${NC}"
if ! psql -d postgres -c '\q' 2>/dev/null; then
    error_exit "Cannot connect to PostgreSQL. Is it running?"
fi
log "${GREEN}✓ PostgreSQL connection OK${NC}\n"

# Setup benchmark database
log "${YELLOW}Setting up benchmark database...${NC}"
if $PSQL -c '\q' 2>/dev/null; then
    log "  Database $DB_NAME already exists - dropping and recreating"
    psql -d postgres -c "DROP DATABASE IF EXISTS $DB_NAME;" || error_exit "Failed to drop database"
    psql -d postgres -c "CREATE DATABASE $DB_NAME;" || error_exit "Failed to create database"
    log "  ${GREEN}✓ Database recreated${NC}"
else
    psql -d postgres -c "CREATE DATABASE $DB_NAME;" || error_exit "Failed to create database"
    log "  ${GREEN}✓ Database created${NC}"
fi

# Run setup
log "\n${YELLOW}Running benchmark setup...${NC}"
$PSQL -f 00_setup.sql > /dev/null || error_exit "Setup failed"
log "${GREEN}✓ Setup complete${NC}\n"

# Function to run a benchmark scenario
run_scenario() {
    local scenario=$1
    local scale=$2
    local scenario_name=$3

    log "${BLUE}=== Running $scenario_name ($scale scale) ===${NC}"

    # Show state before cleanup (if DEBUG mode enabled)
    if [ "$DEBUG" = "true" ]; then
        log "Before cleanup:"
        check_database_state
    fi

    # Clean up previous scenario
    log "  Cleaning up previous scenario..."
    $PSQL -c "DROP SCHEMA IF EXISTS benchmark CASCADE; CREATE SCHEMA benchmark; SET search_path TO benchmark, public;" \
        || error_exit "Schema cleanup failed"
    log "  ${GREEN}✓ Cleanup complete${NC}"

    # Show state after cleanup (if DEBUG mode enabled)
    if [ "$DEBUG" = "true" ]; then
        log "After cleanup:"
        check_database_state
    fi

    # Load schema
    log "  Loading schema..."
    # Set search_path before loading schema
    $PSQL -c "SET search_path TO benchmark, public;" || error_exit "Failed to set search_path"
    $PSQL -f "schemas/${scenario}_schema.sql" || error_exit "Schema load failed for $scenario"

    # Generate data
    log "  Generating $scale scale data..."
    # Substitute the data scale directly into the SQL file
    sed "s/__DATA_SCALE__/'$scale'/g" "data/${scenario}_data.sql" | $PSQL 2>&1 | grep -E "NOTICE|ERROR"

    # Run benchmarks
    log "  Running benchmarks..."
    $PSQL -c "SET search_path TO benchmark, public;" -v data_scale="$scale" -f "scenarios/${scenario}_benchmarks.sql"

    log "${GREEN}✓ $scenario_name ($scale) complete${NC}\n"
}

# Parse command line arguments
SCENARIOS="ecommerce"
SCALES="small medium large"
RUN_ALL=true

while [[ $# -gt 0 ]]; do
    case $1 in
        --scenario)
            SCENARIOS="$2"
            RUN_ALL=false
            shift 2
            ;;
        --scale)
            SCALES="$2"
            RUN_ALL=false
            shift 2
            ;;
        --help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --scenario SCENARIO   Run specific scenario (ecommerce)"
            echo "  --scale SCALE        Run specific scale (small, medium, large)"
            echo "  --help               Show this help message"
            echo ""
            echo "Examples:"
            echo "  $0                                    # Run all scenarios at all scales"
            echo "  $0 --scenario ecommerce --scale small  # Run e-commerce small only"
            exit 0
            ;;
        *)
            error_exit "Unknown option: $1\nUse --help for usage information"
            ;;
    esac
done

# Run benchmarks
START_TIME=$(date +%s)

for scenario in $SCENARIOS; do
    for scale in $SCALES; do
        case $scenario in
            ecommerce)
                run_scenario "01_ecommerce" "$scale" "E-Commerce"
                ;;
            *)
                log "${RED}Unknown scenario: $scenario${NC}"
                ;;
        esac
    done
done

END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

# Generate summary report
log "\n${BLUE}=========================================${NC}"
log "${BLUE}Benchmark Summary${NC}"
log "${BLUE}=========================================${NC}\n"

log "Generating summary report..."
$PSQL -c "
    SELECT
        scenario,
        data_scale,
        test_name,
        operation_type,
        rows_affected,
        ROUND(execution_time_ms, 2) as time_ms,
        ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) as ms_per_row
    FROM benchmark_results
    ORDER BY scenario, data_scale, test_name, operation_type;
" | tee -a "$LOG_FILE"

log "\n${YELLOW}Performance Improvements:${NC}"
$PSQL -c "
    SELECT
        scenario,
        data_scale,
        test_name,
        operation_type as incremental_type,
        rows_affected,
        ROUND(baseline_ms, 2) as full_refresh_ms,
        ROUND(incremental_ms, 2) as incremental_ms,
        improvement_ratio || 'x faster' as improvement,
        ROUND(time_saved_ms, 2) as saved_ms
    FROM benchmark_comparison
    WHERE improvement_ratio IS NOT NULL
    ORDER BY improvement_ratio DESC;
" | tee -a "$LOG_FILE"

# Export to CSV
CSV_FILE="$RESULTS_DIR/benchmark_results_$TIMESTAMP.csv"
log "\n${YELLOW}Exporting results to CSV: $CSV_FILE${NC}"
$PSQL -c "\COPY benchmark_results TO '$CSV_FILE' WITH CSV HEADER;" || log "${RED}CSV export failed${NC}"

log "\n${GREEN}=========================================${NC}"
log "${GREEN}Benchmarks Complete!${NC}"
log "${GREEN}=========================================${NC}"
log "Total duration: ${DURATION}s"
log "Results logged to: $LOG_FILE"
log "CSV results: $CSV_FILE"
log ""

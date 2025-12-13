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
mkdir -p "$RESULTS_DIR"

echo -e "${BLUE}=========================================${NC}"
echo -e "${BLUE}pg_tviews Comprehensive Benchmark Suite${NC}"
echo -e "${BLUE}=========================================${NC}"
echo ""
echo "Started at: $(date)"
echo "Results will be saved to: $LOG_FILE"
echo ""

# ============================================================================
# Logging Functions
# ============================================================================

log() {
    echo -e "$1" | tee -a "$LOG_FILE"
}

log_info() {
    log "[$(date '+%Y-%m-%d %H:%M:%S')] ℹ️  INFO: $1"
}

log_success() {
    log "[$(date '+%Y-%m-%d %H:%M:%S')] ✅ SUCCESS: $1"
}

log_error() {
    log "[$(date '+%Y-%m-%d %H:%M:%S')] ❌ ERROR: $1"
}

log_warning() {
    log "[$(date '+%Y-%m-%d %H:%M:%S')] ⚠️  WARNING: $1"
}

log_step() {
    log "[$(date '+%Y-%m-%d %H:%M:%S')] 📍 STEP: $1"
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

    log_step "Starting $scenario_name ($scale scale) benchmark"

    # Show state before cleanup (if DEBUG mode enabled)
    if [ "$DEBUG" = "true" ]; then
        log_info "Checking database state before cleanup"
        check_database_state
    fi

    # Clean up previous scenario
    log_step "Cleaning up previous scenario"
    if $PSQL -c "DROP SCHEMA IF EXISTS benchmark CASCADE; CREATE SCHEMA benchmark; SET search_path TO benchmark, public;" 2>&1 | tee -a "$LOG_FILE"; then
        log_success "Schema cleanup completed"
    else
        log_error "Schema cleanup failed"
        return 1
    fi

    # Show state after cleanup (if DEBUG mode enabled)
    if [ "$DEBUG" = "true" ]; then
        log_info "Checking database state after cleanup"
        check_database_state
    fi

    # Load schema
    log_step "Loading database schema"
    if $PSQL -c "SET search_path TO benchmark, public;" 2>&1 | tee -a "$LOG_FILE"; then
        log_info "Search path set successfully"
    else
        log_error "Failed to set search_path"
        return 1
    fi

    if $PSQL -f "schemas/${scenario}_schema.sql" 2>&1 | tee -a "$LOG_FILE"; then
        log_success "Schema loaded successfully"

        # Verify schema
        local table_count=$($PSQL -t -c "SELECT COUNT(*) FROM pg_tables WHERE schemaname = 'benchmark';")
        log_info "Found $table_count tables in benchmark schema"
    else
        log_error "Schema loading failed"
        return 1
    fi

    # Generate data
    log_step "Generating $scale scale data"
    if $PSQL -v data_scale="$scale" -f "data/${scenario}_data.sql" 2>&1 | grep -E "NOTICE|ERROR" | tee -a "$LOG_FILE"; then
        log_success "Data generation completed"

        # Verify data
        local product_count=$($PSQL -t -c "SELECT COUNT(*) FROM benchmark.tb_product;")
        log_info "Loaded $product_count products"
    else
        log_error "Data generation failed"
        return 1
    fi

    # Run benchmarks
    log_step "Running benchmark scenarios"
    if $PSQL -v data_scale="$scale" -f "scenarios/${scenario}_benchmarks.sql" 2>&1 | tee -a "$LOG_FILE"; then
        log_success "Benchmark scenarios completed"
    else
        log_error "Benchmark scenarios failed"
        return 1
    fi

    log_success "$scenario_name ($scale) benchmark completed successfully"
    echo ""
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
log_info "Starting benchmark run at $(date)"
log_info "Configuration: scenarios=$SCENARIOS, scales=$SCALES"

for scenario in $SCENARIOS; do
    for scale in $SCALES; do
        case $scenario in
            ecommerce)
                run_scenario "01_ecommerce" "$scale" "E-Commerce"
                ;;
            *)
                log_error "Unknown scenario: $scenario"
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

log_info "Benchmark run summary:"
log_info "  Scenarios: $SCENARIOS"
log_info "  Scales: $SCALES"
log_info "  Total time: ${DURATION}s"
log_success "All benchmarks completed successfully"
log_info "Results logged to: $LOG_FILE"
log_info "CSV results: $CSV_FILE"
echo ""

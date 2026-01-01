#!/bin/bash
# Run 4-way performance comparison benchmarks
# Compares: pg_tviews+jsonb_delta, pg_tviews+native, manual_func, full_refresh

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
DB_NAME="${PGDATABASE:-pg_tviews_benchmark}"
PSQL="psql -d $DB_NAME -v ON_ERROR_STOP=1"
RESULTS_DIR="results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOG_FILE="$RESULTS_DIR/4way_comparison_$TIMESTAMP.log"

# Create results directory
mkdir -p "$RESULTS_DIR"

echo -e "${BLUE}=======================================${NC}"
echo -e "${BLUE}4-WAY PERFORMANCE COMPARISON${NC}"
echo -e "${BLUE}=======================================${NC}"
echo ""
echo "Database: $DB_NAME"
echo "Log file: $LOG_FILE"
echo ""

# Parse command line arguments
SCALES="small medium large"

while [[ $# -gt 0 ]]; do
    case $1 in
        --scale)
            SCALES="$2"
            shift 2
            ;;
        --db)
            DB_NAME="$2"
            shift 2
            ;;
        --help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --scale SCALE    Run specific scale (small, medium, large) [default: all]"
            echo "  --db DATABASE    Database name [default: pg_tviews_benchmark]"
            echo "  --help           Show this help message"
            echo ""
            echo "Examples:"
            echo "  $0                      # Run all scales"
            echo "  $0 --scale small        # Run small scale only"
            echo "  $0 --scale 'small medium'  # Run small and medium"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Check PostgreSQL connection
echo -e "${YELLOW}Checking PostgreSQL connection...${NC}"
if ! psql -d postgres -c '\q' 2>/dev/null; then
    echo -e "${RED}Cannot connect to PostgreSQL. Is it running?${NC}"
    exit 1
fi
echo -e "${GREEN}✓ PostgreSQL connection OK${NC}"
echo ""

# Setup benchmark database
echo -e "${YELLOW}Setting up benchmark database...${NC}"
if $PSQL -c '\q' 2>/dev/null; then
    echo "  Database $DB_NAME already exists - dropping and recreating..."
    psql -d postgres -c "DROP DATABASE IF EXISTS $DB_NAME;" || exit 1
    psql -d postgres -c "CREATE DATABASE $DB_NAME;" || exit 1
    echo -e "  ${GREEN}✓ Database recreated${NC}"
else
    psql -d postgres -c "CREATE DATABASE $DB_NAME;" || exit 1
    echo -e "  ${GREEN}✓ Database created${NC}"
fi

# Run setup
echo ""
echo -e "${YELLOW}Running benchmark setup...${NC}"
$PSQL -f 00_setup.sql > /dev/null || exit 1
echo -e "${GREEN}✓ Setup complete${NC}"
echo ""

# Run benchmarks for each scale
START_TIME=$(date +%s)

for scale in $SCALES; do
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}Running $scale scale comparison${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""

    $PSQL -v data_scale="'$scale'" -f scenarios/04_way_comparison.sql 2>&1 | tee -a "$LOG_FILE"

    echo ""
done

END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

# Generate final summary
echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}FINAL SUMMARY - ALL SCALES${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

$PSQL -c "
-- Summary across all scales
SELECT
    data_scale AS scale,
    test_name AS operation,
    operation_type AS approach,
    ROUND(execution_time_ms, 2) AS time_ms,
    rows_affected,
    ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) AS ms_per_row
FROM benchmark_results
WHERE scenario = 'ecommerce'
ORDER BY data_scale, test_name, execution_time_ms;
" | tee -a "$LOG_FILE"

echo ""
echo -e "${BLUE}--- Cross-Scale Performance ---${NC}"
echo ""

$PSQL -c "
-- Show how each approach scales
WITH perf AS (
    SELECT
        operation_type,
        test_name,
        data_scale,
        execution_time_ms,
        rows_affected,
        ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) AS ms_per_row
    FROM benchmark_results
    WHERE scenario = 'ecommerce'
)
SELECT
    operation_type AS approach,
    test_name AS operation,
    MAX(CASE WHEN data_scale = 'small' THEN ROUND(ms_per_row, 3) END) AS small_ms_per_row,
    MAX(CASE WHEN data_scale = 'medium' THEN ROUND(ms_per_row, 3) END) AS medium_ms_per_row,
    MAX(CASE WHEN data_scale = 'large' THEN ROUND(ms_per_row, 3) END) AS large_ms_per_row,
    ROUND(
        MAX(CASE WHEN data_scale = 'large' THEN ms_per_row END) /
        NULLIF(MAX(CASE WHEN data_scale = 'small' THEN ms_per_row END), 0),
        2
    ) AS scaling_factor
FROM perf
GROUP BY operation_type, test_name
ORDER BY test_name, operation_type;
" | tee -a "$LOG_FILE"

# Export to CSV
echo ""
echo -e "${YELLOW}Exporting results to CSV...${NC}"
CSV_FILE="$RESULTS_DIR/4way_comparison_$TIMESTAMP.csv"
$PSQL -c "\COPY (
    SELECT
        data_scale,
        test_name,
        operation_type,
        rows_affected,
        ROUND(execution_time_ms, 2) AS execution_time_ms,
        ROUND(execution_time_ms / NULLIF(rows_affected, 0), 3) AS ms_per_row,
        notes
    FROM benchmark_results
    WHERE scenario = 'ecommerce'
    ORDER BY data_scale, test_name, operation_type
) TO '$CSV_FILE' WITH CSV HEADER;"

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}BENCHMARKS COMPLETE!${NC}"
echo -e "${GREEN}========================================${NC}"
echo "Total duration: ${DURATION}s"
echo "Results logged to: $LOG_FILE"
echo "CSV results: $CSV_FILE"
echo ""
echo -e "${BLUE}Key Findings:${NC}"
echo ""

# Show winner for each operation
$PSQL -t -c "
WITH ranked AS (
    SELECT
        data_scale,
        test_name,
        operation_type,
        execution_time_ms,
        ROW_NUMBER() OVER (PARTITION BY data_scale, test_name ORDER BY execution_time_ms) AS rank
    FROM benchmark_results
    WHERE scenario = 'ecommerce'
)
SELECT
    data_scale || ' / ' || test_name || ': ' ||
    operation_type || ' (' || ROUND(execution_time_ms, 2) || ' ms)' AS winner
FROM ranked
WHERE rank = 1
ORDER BY data_scale, test_name;
" | tee -a "$LOG_FILE"

echo ""
echo -e "${YELLOW}View full results with:${NC}"
echo "  psql -d $DB_NAME -c 'SELECT * FROM benchmark_summary;'"
echo "  psql -d $DB_NAME -c 'SELECT * FROM benchmark_comparison;'"
echo ""

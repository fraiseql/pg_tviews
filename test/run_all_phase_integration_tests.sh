#!/bin/bash
# Comprehensive Integration Test Runner for All Phases
# Tests all implemented features end-to-end

set -e

echo "🧪 Starting Comprehensive Integration Tests for All Phases"
echo "=========================================================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to run a test and report result
run_test() {
    local test_name="$1"
    local test_file="$2"

    echo -e "${BLUE}Running: ${test_name}${NC}"

    if psql -f "$test_file" --quiet 2>/dev/null; then
        echo -e "${GREEN}✅ PASSED: ${test_name}${NC}"
        return 0
    else
        echo -e "${RED}❌ FAILED: ${test_name}${NC}"
        echo -e "${YELLOW}   Check output above for details${NC}"
        return 1
    fi
}

# Function to check if extension is loaded
check_extension() {
    echo "Checking pg_tviews extension..."
    if ! psql -c "SELECT pg_tviews_health_check();" --quiet >/dev/null 2>&1; then
        echo -e "${RED}❌ pg_tviews extension not loaded or not working${NC}"
        exit 1
    fi
    echo -e "${GREEN}✅ pg_tviews extension is ready${NC}"
}

# Main test execution
main() {
    local failed_tests=0

    # Pre-flight checks
    check_extension

    echo -e "\n${YELLOW}Phase 1: Savepoint Depth Tracking${NC}"
    if run_test "Savepoint Depth Integration" "test/sql/phase_1_savepoint_depth_integration.sql"; then
        echo "Savepoint depth tracking works correctly"
    else
        ((failed_tests++))
    fi

    echo -e "\n${YELLOW}Phase 2: GUC Configuration System${NC}"
    if run_test "GUC Configuration Integration" "test/sql/phase_2_guc_configuration_integration.sql"; then
        echo "GUC configuration system works correctly"
    else
        ((failed_tests++))
    fi

    echo -e "\n${YELLOW}Phase 3: Queue Introspection${NC}"
    if run_test "Queue Introspection Integration" "test/sql/phase_3_queue_introspection_integration.sql"; then
        echo "Queue introspection works correctly"
    else
        ((failed_tests++))
    fi

    echo -e "\n${YELLOW}Phase 4: Dynamic Primary Key Detection${NC}"
    if run_test "Dynamic PK Detection Integration" "test/sql/phase_4_dynamic_pk_detection_integration.sql"; then
        echo "Dynamic primary key detection works correctly"
    else
        ((failed_tests++))
    fi

    echo -e "\n${YELLOW}Phase 5: Cached Plan Refresh Integration${NC}"
    if run_test "Cached Plan Refresh Integration" "test/sql/phase_5_cached_plan_refresh_integration.sql"; then
        echo "Cached plan refresh integration works correctly"
    else
        ((failed_tests++))
    fi

    echo -e "\n${YELLOW}Phase 6: TEXT[][] Extraction Workaround${NC}"
    if run_test "TEXT[][] Extraction Integration" "test/sql/phase_6_text_array_extraction_integration.sql"; then
        echo "TEXT[][] extraction workaround works correctly"
    else
        ((failed_tests++))
    fi

    # Summary
    echo -e "\n=========================================================="
    if [ $failed_tests -eq 0 ]; then
        echo -e "${GREEN}🎉 ALL INTEGRATION TESTS PASSED!${NC}"
        echo -e "${GREEN}All phases are working correctly in PostgreSQL environment.${NC}"
        exit 0
    else
        echo -e "${RED}❌ $failed_tests INTEGRATION TEST(S) FAILED${NC}"
        echo -e "${YELLOW}Check the output above for failure details.${NC}"
        echo -e "${YELLOW}Some phases may need debugging or fixes.${NC}"
        exit 1
    fi
}

# Run main function
main "$@"
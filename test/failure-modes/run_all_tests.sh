#!/bin/bash
# Master test runner for pg_tviews failure modes test suite

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_FILE="$SCRIPT_DIR/test_results_$(date +%Y%m%d_%H%M%S).log"

echo "pg_tviews Failure Modes Test Suite"
echo "==================================="
echo "Log file: $LOG_FILE"
echo ""

# Initialize log
echo "pg_tviews Failure Modes Test Results" > "$LOG_FILE"
echo "Started: $(date)" >> "$LOG_FILE"
echo "===================================" >> "$LOG_FILE"

TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0

run_test() {
    local test_name="$1"
    local test_script="$2"

    echo -n "Running $test_name... "
    TOTAL_TESTS=$((TOTAL_TESTS + 1))

    if [ -x "$test_script" ]; then
        if "$test_script" >> "$LOG_FILE" 2>&1; then
            echo "✅ PASSED"
            PASSED_TESTS=$((PASSED_TESTS + 1))
        else
            echo "❌ FAILED"
            FAILED_TESTS=$((FAILED_TESTS + 1))
        fi
    else
        echo "⚠️  SKIPPED (not executable)"
        echo "$test_name: SKIPPED (not executable)" >> "$LOG_FILE"
    fi
}

echo "Database Failure Tests"
echo "----------------------"
run_test "Crash Recovery" "$SCRIPT_DIR/db-failures/test-crash-recovery.sh"
run_test "Disk Full" "$SCRIPT_DIR/db-failures/test-disk-full.sh"
run_test "Out of Memory" "$SCRIPT_DIR/db-failures/test-oom.sh"

echo ""
echo "Extension Failure Tests"
echo "-----------------------"
run_test "Circular Dependencies" "$SCRIPT_DIR/extension-failures/test-circular-deps.sh"
run_test "Metadata Corruption" "$SCRIPT_DIR/extension-failures/test-metadata-corruption.sh"
run_test "Queue Corruption" "$SCRIPT_DIR/extension-failures/test-queue-corruption.sh"

echo ""
echo "Operational Failure Tests"
echo "-------------------------"
run_test "PostgreSQL Upgrade" "$SCRIPT_DIR/operational/test-upgrade.sh"
run_test "Backup/Restore" "$SCRIPT_DIR/operational/test-backup-restore.sh"
run_test "Concurrent DDL" "$SCRIPT_DIR/operational/test-concurrent-ddl.sh"

echo ""
echo "Summary"
echo "======="
echo "Total tests: $TOTAL_TESTS"
echo "Passed: $PASSED_TESTS"
echo "Failed: $FAILED_TESTS"

echo ""
echo "Total tests: $TOTAL_TESTS" >> "$LOG_FILE"
echo "Passed: $PASSED_TESTS" >> "$LOG_FILE"
echo "Failed: $FAILED_TESTS" >> "$LOG_FILE"
echo "Finished: $(date)" >> "$LOG_FILE"

if [ $FAILED_TESTS -eq 0 ]; then
    echo "🎉 All tests passed!"
    echo "🎉 All tests passed!" >> "$LOG_FILE"
    exit 0
else
    echo "❌ $FAILED_TESTS test(s) failed. Check log: $LOG_FILE"
    exit 1
fi
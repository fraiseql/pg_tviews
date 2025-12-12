#!/bin/bash
set -euo pipefail

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="/tmp/pg_tviews_migration_logs"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
MASTER_LOG="$LOG_DIR/master_${TIMESTAMP}.log"

mkdir -p "$LOG_DIR"

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Logging function
log() {
    echo -e "$1" | tee -a "$MASTER_LOG"
}

log_success() {
    log "${GREEN}✓ $1${NC}"
}

log_error() {
    log "${RED}✗ ERROR: $1${NC}"
}

log_warning() {
    log "${YELLOW}⚠ WARNING: $1${NC}"
}

# Error handler
handle_error() {
    local step=$1
    local exit_code=$2

    log_error "Step failed: $step (exit code: $exit_code)"
    log ""
    log "Check logs at: $MASTER_LOG"
    log ""

    # Save failure metadata
    cat > "/tmp/migration_failure_${TIMESTAMP}.json" <<EOF
{
  "timestamp": "$(date -Iseconds)",
  "failed_step": "$step",
  "exit_code": $exit_code,
  "git_sha": "$(git rev-parse HEAD 2>/dev/null || echo 'unknown')",
  "log_file": "$MASTER_LOG"
}
EOF

    # Cleanup on failure (optional)
    if [[ "${CLEANUP_ON_FAILURE:-true}" == "true" ]]; then
        log "Cleaning up..."
        "$SCRIPT_DIR/02_cleanup.sh" partial 2>&1 | tee -a "$MASTER_LOG" || true
    fi

    exit $exit_code
}

# Trap errors
trap 'handle_error "Unknown" $?' ERR

# Banner
log "=========================================="
log "  PG_TVIEWS PODMAN MIGRATION"
log "=========================================="
log ""
log "Timestamp: $(date)"
log "Log file: $MASTER_LOG"
log ""

# Step 1: Pre-flight checks
log "=========================================="
log "STEP 1: PRE-FLIGHT CHECKS"
log "=========================================="
log ""

if ! "$SCRIPT_DIR/01_preflight_checks.sh" 2>&1 | tee -a "$MASTER_LOG"; then
    handle_error "Pre-flight checks" $?
fi

log_success "Pre-flight checks passed"
log ""

# Step 2: Cleanup
log "=========================================="
log "STEP 2: CLEANUP OLD STATE"
log "=========================================="
log ""

CLEANUP_MODE="${CLEANUP_MODE:-partial}"
if ! "$SCRIPT_DIR/02_cleanup.sh" "$CLEANUP_MODE" 2>&1 | tee -a "$MASTER_LOG"; then
    handle_error "Cleanup" $?
fi

log_success "Cleanup complete"
log ""

# Step 3: Build
log "=========================================="
log "STEP 3: BUILD IMAGE"
log "=========================================="
log ""

if ! "$SCRIPT_DIR/03_build.sh" 2>&1 | tee -a "$MASTER_LOG"; then
    handle_error "Build" $?
fi

log_success "Build complete"
log ""

# Step 4: Smoke test
log "=========================================="
log "STEP 4: SMOKE TEST"
log "=========================================="
log ""

if ! "$SCRIPT_DIR/04_smoke_test.sh" 2>&1 | tee -a "$MASTER_LOG"; then
    handle_error "Smoke test" $?
fi

log_success "Smoke test passed"
log ""

# Step 5: Run benchmarks
log "=========================================="
log "STEP 5: RUN BENCHMARKS"
log "=========================================="
log ""

if ! "$SCRIPT_DIR/05_run_benchmarks.sh" 2>&1 | tee -a "$MASTER_LOG"; then
    handle_error "Benchmarks" $?
fi

log_success "Benchmarks complete"
log ""

# Step 6: Collect artifacts
log "=========================================="
log "STEP 6: COLLECT ARTIFACTS"
log "=========================================="
log ""

if ! "$SCRIPT_DIR/06_collect_artifacts.sh" 2>&1 | tee -a "$MASTER_LOG"; then
    handle_error "Artifact collection" $?
fi

log_success "Artifacts collected"
log ""

# Step 7: Final cleanup (optional for CI)
if [[ "${CLEANUP_AFTER_RUN:-false}" == "true" ]]; then
    log "=========================================="
    log "STEP 7: FINAL CLEANUP"
    log "=========================================="
    log ""

    "$SCRIPT_DIR/02_cleanup.sh" full 2>&1 | tee -a "$MASTER_LOG" || true

    log_success "Final cleanup complete"
    log ""
fi

# Success summary
log "=========================================="
log "  MIGRATION COMPLETE - ALL STEPS PASSED"
log "=========================================="
log ""
log_success "All steps completed successfully"
log ""
log "Master log: $MASTER_LOG"
log ""

# Generate success report
cat > "/tmp/migration_success_${TIMESTAMP}.json" <<EOF
{
  "timestamp": "$(date -Iseconds)",
  "status": "success",
  "git_sha": "$(git rev-parse HEAD)",
  "git_branch": "$(git branch --show-current)",
  "duration_seconds": $SECONDS,
  "log_file": "$MASTER_LOG",
  "artifacts": "$(cat /tmp/artifact_paths.txt 2>/dev/null | jq -R . | jq -s . || echo '[]')"
}
EOF

log "Success report: /tmp/migration_success_${TIMESTAMP}.json"
log ""

exit 0

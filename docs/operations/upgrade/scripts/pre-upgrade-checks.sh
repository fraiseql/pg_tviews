#!/bin/bash
# pg_tviews Pre-Upgrade Checks Script
# Comprehensive validation before upgrade operations
# Usage: ./pre-upgrade-checks.sh [database_name] [host] [user]

set -euo pipefail

# Configuration
DB_NAME="${1:-${DB_NAME:-pg_tviews_db}}"
DB_HOST="${2:-${DB_HOST:-localhost}}"
DB_USER="${3:-${DB_USER:-postgres}}"
DB_PORT="${DB_PORT:-5432}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Database connection test
test_connection() {
    log_info "Testing database connection..."
    if psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -c "SELECT 1;" >/dev/null 2>&1; then
        log_success "Database connection successful"
        return 0
    else
        log_error "Cannot connect to database $DB_NAME on $DB_HOST:$DB_PORT as $DB_USER"
        return 1
    fi
}

# PostgreSQL version check
check_postgres_version() {
    log_info "Checking PostgreSQL version..."
    local version
    version=$(psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -tAc "SELECT version();")

    if [[ $version == *"PostgreSQL 15."* ]] || [[ $version == *"PostgreSQL 16."* ]]; then
        log_success "PostgreSQL version compatible: $version"
        return 0
    else
        log_warning "PostgreSQL version may not be fully tested: $version"
        return 0
    fi
}

# pg_tviews extension check
check_extension() {
    log_info "Checking pg_tviews extension..."
    local ext_version
    ext_version=$(psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -tAc "SELECT pg_tviews_version();" 2>/dev/null || echo "NOT_INSTALLED")

    if [[ $ext_version == "NOT_INSTALLED" ]]; then
        log_error "pg_tviews extension not installed"
        return 1
    else
        log_success "pg_tviews extension installed: $ext_version"
        return 0
    fi
}

# TVIEW health check
check_tview_health() {
    log_info "Checking TVIEW health..."

    # Count TVIEWs
    local tview_count
    tview_count=$(psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -tAc "SELECT COUNT(*) FROM pg_tviews_metadata;")

    # Count healthy TVIEWs
    local healthy_count
    healthy_count=$(psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -tAc "SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NULL;")

    # Count TVIEWs with errors
    local error_count
    error_count=$(psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -tAc "SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL;")

    log_info "TVIEWs found: $tview_count (Healthy: $healthy_count, Errors: $error_count)"

    if [[ $error_count -gt 0 ]]; then
        log_warning "$error_count TVIEWs have errors - review before upgrade"
        # Show error details
        psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -c "
        SELECT entity_name, LEFT(last_error, 100) as error_preview
        FROM pg_tviews_metadata
        WHERE last_error IS NOT NULL
        LIMIT 5;
        " | head -10
    fi

    if [[ $healthy_count -eq $tview_count ]]; then
        log_success "All TVIEWs are healthy"
        return 0
    else
        log_warning "Some TVIEWs have issues - consider resolving before upgrade"
        return 0
    fi
}

# Queue status check
check_queue_status() {
    log_info "Checking refresh queue status..."

    local pending_count
    pending_count=$(psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -tAc "SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL;")

    local oldest_pending
    oldest_pending=$(psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -tAc "SELECT EXTRACT(EPOCH FROM (NOW() - MIN(created_at))) FROM pg_tviews_queue WHERE processed_at IS NULL;" 2>/dev/null || echo "0")

    log_info "Pending refresh items: $pending_count"

    if [[ $pending_count -gt 100 ]]; then
        log_warning "High number of pending refresh items ($pending_count)"
    fi

    if [[ $(echo "$oldest_pending > 3600" | bc -l 2>/dev/null || echo "0") -eq 1 ]]; then
        log_warning "Some pending items older than 1 hour"
    fi

    if [[ $pending_count -le 100 ]] && [[ $(echo "$oldest_pending <= 3600" | bc -l 2>/dev/null || echo "1") -eq 1 ]]; then
        log_success "Queue status acceptable"
    fi
}

# Disk space check
check_disk_space() {
    log_info "Checking disk space..."

    # Get PostgreSQL data directory
    local data_dir
    data_dir=$(psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -tAc "SHOW data_directory;")

    # Check available space (requires df command)
    if command -v df >/dev/null 2>&1; then
        local available_gb
        available_gb=$(df -BG "$data_dir" | tail -1 | awk '{print $4}' | sed 's/G//')

        if [[ $available_gb -gt 20 ]]; then
            log_success "Sufficient disk space available: ${available_gb}GB"
        else
            log_warning "Limited disk space available: ${available_gb}GB (recommend 20GB+ for upgrades)"
        fi
    else
        log_info "Cannot check disk space (df command not available)"
    fi
}

# Backup verification
check_backup_readiness() {
    log_info "Checking backup readiness..."

    # Check if pg_basebackup is available
    if command -v pg_basebackup >/dev/null 2>&1; then
        log_success "pg_basebackup available for backups"
    else
        log_warning "pg_basebackup not available - ensure backup tools are ready"
    fi

    # Check if backup directory exists and is writable
    local backup_dir="${BACKUP_DIR:-/tmp/backups}"
    if [[ -w "$(dirname "$backup_dir")" ]]; then
        log_success "Backup directory writable: $(dirname "$backup_dir")"
    else
        log_warning "Backup directory may not be writable: $(dirname "$backup_dir")"
    fi
}

# System resource check
check_system_resources() {
    log_info "Checking system resources..."

    # Check memory
    if command -v free >/dev/null 2>&1; then
        local total_mem_gb
        total_mem_gb=$(free -g | grep '^Mem:' | awk '{print $2}')

        if [[ $total_mem_gb -gt 8 ]]; then
            log_success "Sufficient memory available: ${total_mem_gb}GB"
        else
            log_warning "Limited memory: ${total_mem_gb}GB (8GB+ recommended for upgrades)"
        fi
    fi

    # Check CPU cores
    if command -v nproc >/dev/null 2>&1; then
        local cpu_cores
        cpu_cores=$(nproc)

        if [[ $cpu_cores -gt 2 ]]; then
            log_success "Sufficient CPU cores: $cpu_cores"
        else
            log_warning "Limited CPU cores: $cpu_cores (4+ recommended for upgrades)"
        fi
    fi
}

# Generate pre-upgrade report
generate_report() {
    log_info "Generating pre-upgrade report..."

    local report_file="/tmp/pg_tviews-pre-upgrade-$(date +%Y%m%d_%H%M%S).txt"

    {
        echo "pg_tviews Pre-Upgrade Check Report"
        echo "=================================="
        echo "Date: $(date)"
        echo "Database: $DB_NAME"
        echo "Host: $DB_HOST:$DB_PORT"
        echo "User: $DB_USER"
        echo ""

        echo "PostgreSQL Version:"
        psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -tAc "SELECT version();" 2>/dev/null || echo "Unable to retrieve"

        echo ""
        echo "pg_tviews Version:"
        psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -tAc "SELECT pg_tviews_version();" 2>/dev/null || echo "Not installed"

        echo ""
        echo "TVIEW Summary:"
        psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -c "
        SELECT
            COUNT(*) as total_tviews,
            COUNT(*) FILTER (WHERE last_error IS NULL) as healthy,
            COUNT(*) FILTER (WHERE last_error IS NOT NULL) as with_errors,
            ROUND(AVG(last_refresh_duration_ms), 0) as avg_refresh_ms
        FROM pg_tviews_metadata;
        " 2>/dev/null || echo "Unable to retrieve TVIEW data"

        echo ""
        echo "Queue Status:"
        psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" -p "$DB_PORT" -c "
        SELECT
            COUNT(*) as total_items,
            COUNT(*) FILTER (WHERE processed_at IS NULL) as pending,
            COUNT(*) FILTER (WHERE error_message IS NOT NULL) as failed
        FROM pg_tviews_queue;
        " 2>/dev/null || echo "Unable to retrieve queue data"

    } > "$report_file"

    log_success "Pre-upgrade report saved to: $report_file"
    echo "Review the report before proceeding with upgrade."
}

# Main execution
main() {
    echo "pg_tviews Pre-Upgrade Checks"
    echo "============================"
    echo "Database: $DB_NAME"
    echo "Host: $DB_HOST:$DB_PORT"
    echo "User: $DB_USER"
    echo ""

    local checks_passed=0
    local total_checks=0

    # Run all checks
    ((total_checks++))
    if test_connection; then
        ((checks_passed++))
    fi

    ((total_checks++))
    if check_postgres_version; then
        ((checks_passed++))
    fi

    ((total_checks++))
    if check_extension; then
        ((checks_passed++))
    fi

    ((total_checks++))
    if check_tview_health; then
        ((checks_passed++))
    fi

    ((total_checks++))
    if check_queue_status; then
        ((checks_passed++))
    fi

    check_disk_space
    check_backup_readiness
    check_system_resources

    generate_report

    echo ""
    echo "Summary: $checks_passed/$total_checks critical checks passed"

    if [[ $checks_passed -eq $total_checks ]]; then
        log_success "All critical checks passed - ready for upgrade"
        exit 0
    else
        log_warning "Some checks failed - review issues before upgrade"
        exit 1
    fi
}

# Run main function
main "$@"
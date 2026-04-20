#!/bin/bash
# convert_tviews.sh
#
# Helper script to automatically convert tv_* tables to TVIEWs
#
# Usage:
#   ./sql/convert_tviews.sh [-d database] [-s schema] [-p]
#
# Options:
#   -d, --database DB     PostgreSQL database (default: postgres)
#   -s, --schema SCHEMA   Schema to scan (default: public)
#   -p, --plan            Dry-run mode (show plan only)
#   -h, --host HOST       PostgreSQL host (default: localhost)
#   -U, --user USER       PostgreSQL user (default: $USER)
#   --help                Show this help

set -e

# Defaults
DB="postgres"
SCHEMA="public"
HOST="localhost"
USER="${LOGNAME}"
DRY_RUN=false
PSQL_EXTRA_ARGS=""

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -d|--database)
            DB="$2"
            shift 2
            ;;
        -s|--schema)
            SCHEMA="$2"
            shift 2
            ;;
        -h|--host)
            HOST="$2"
            shift 2
            ;;
        -U|--user)
            USER="$2"
            shift 2
            ;;
        -p|--plan)
            DRY_RUN=true
            shift
            ;;
        --help)
            grep "^#" "$0" | grep -v "^#!/" | sed 's/# //'
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "═══════════════════════════════════════════════════════════════"
echo "  Automatic TVIEW Conversion Tool"
echo "═══════════════════════════════════════════════════════════════"
echo "Database:  $DB"
echo "Schema:    $SCHEMA"
echo "Host:      $HOST"
echo "User:      $USER"
echo ""

# Load the auto-conversion functions
echo "Loading auto-conversion functions..."
psql -h "$HOST" -U "$USER" -d "$DB" -f "$SCRIPT_DIR/auto_convert_tviews.sql" > /dev/null 2>&1
echo "✓ Functions loaded"
echo ""

if [ "$DRY_RUN" = true ]; then
    echo "DRY RUN: Showing conversion plan..."
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    psql -h "$HOST" -U "$USER" -d "$DB" \
        -c "SELECT entity, base_table, backing_view FROM pg_tviews_auto_convert_plan('$SCHEMA') ORDER BY entity;" \
        || true
else
    echo "Executing automatic conversion..."
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    psql -h "$HOST" -U "$USER" -d "$DB" \
        -c "SELECT entity, status, message FROM pg_tviews_auto_convert('$SCHEMA') ORDER BY entity;" \
        || true
fi

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "Verifying TVIEW registration..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
psql -h "$HOST" -U "$USER" -d "$DB" \
    -c "SELECT entity_name, tview_oid::text AS oid FROM pg_tview_meta ORDER BY entity_name LIMIT 10;" \
    || true

TOTAL=$(psql -h "$HOST" -U "$USER" -d "$DB" -t -c "SELECT COUNT(*) FROM pg_tview_meta;" || echo "0")
echo ""
echo "Total TVIEWs registered: $TOTAL"
echo "═══════════════════════════════════════════════════════════════"

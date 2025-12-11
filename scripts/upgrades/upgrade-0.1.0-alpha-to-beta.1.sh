#!/bin/bash
# pg_tviews Upgrade Script: 0.1.0-alpha → 0.1.0-beta.1
# Run this script to upgrade from alpha to beta.1

set -e

DB_NAME="${1:-postgres}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Upgrading pg_tviews from 0.1.0-alpha to 0.1.0-beta.1"
echo "Database: $DB_NAME"
echo

# Check current version
echo "Checking current version..."
CURRENT_VERSION=$(psql -d "$DB_NAME" -t -c "SELECT pg_tviews_version();" 2>/dev/null || echo "not_installed")

if [ "$CURRENT_VERSION" = "not_installed" ]; then
    echo "ERROR: pg_tviews extension not installed"
    exit 1
fi

echo "Current version: $CURRENT_VERSION"

if [ "$CURRENT_VERSION" = "0.1.0-beta.1" ]; then
    echo "Already at target version. Nothing to do."
    exit 0
fi

# Backup recommendation
echo "⚠️  IMPORTANT: Ensure you have a backup before proceeding!"
echo "Run: pg_dump -Fc $DB_NAME > backup_$(date +%Y%m%d_%H%M%S).dump"
echo
read -p "Do you have a backup? (y/N): " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Please create a backup first."
    exit 1
fi

# Perform upgrade
echo "Performing upgrade..."

# For alpha to beta.1, we can do an in-place upgrade
psql -d "$DB_NAME" -c "ALTER EXTENSION pg_tviews UPDATE;" 2>/dev/null || {
    echo "In-place upgrade failed. Performing full recreation..."

    # Full recreation path
    psql -d "$DB_NAME" -c "DROP EXTENSION pg_tviews;" 2>/dev/null || true

    # Reinstall would happen here (assume user has done this)
    echo "Please reinstall pg_tviews extension and recreate TVIEWs manually."
    exit 1
}

# Verify upgrade
NEW_VERSION=$(psql -d "$DB_NAME" -t -c "SELECT pg_tviews_version();")
echo "New version: $NEW_VERSION"

if [ "$NEW_VERSION" = "0.1.0-beta.1" ]; then
    echo "✅ Upgrade successful!"

    # Run health check
    echo "Running health check..."
    psql -d "$DB_NAME" -c "SELECT * FROM pg_tviews_health_check();" || echo "Health check failed - please verify manually"

else
    echo "❌ Upgrade verification failed"
    exit 1
fi

echo
echo "Upgrade complete. Please test your application thoroughly."
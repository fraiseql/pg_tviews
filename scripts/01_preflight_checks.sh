#!/bin/bash
set -euo pipefail

# Configuration
MIN_DISK_GB=10
MIN_MEM_GB=4
REQUIRED_TOOLS=("podman" "git" "jq")

echo "=== PRE-FLIGHT CHECKS ==="

# Function: Check command exists
check_command() {
    if ! command -v "$1" &> /dev/null; then
        echo "ERROR: Required tool '$1' not found"
        exit 1
    fi
    echo "✓ $1 installed: $(command -v $1)"
}

# Check required tools
for tool in "${REQUIRED_TOOLS[@]}"; do
    check_command "$tool"
done

# Check Podman version (need 4.0+)
PODMAN_VERSION=$(podman --version | awk '{print $3}' | cut -d. -f1)
if [[ $PODMAN_VERSION -lt 4 ]]; then
    echo "ERROR: Podman version 4.0+ required (found: $(podman --version))"
    exit 1
fi
echo "✓ Podman version: $(podman --version)"

# Check cgroups v2
CGROUP_VERSION=$(stat -fc %T /sys/fs/cgroup/)
if [[ "$CGROUP_VERSION" != "cgroup2fs" ]]; then
    echo "WARNING: cgroups v2 not detected. Rootless mode may have issues."
else
    echo "✓ cgroups v2 enabled"
fi

# Check disk space
STORAGE_PATH="${XDG_DATA_HOME:-$HOME/.local/share}/containers/storage"
mkdir -p "$STORAGE_PATH" 2>/dev/null || true

AVAILABLE_SPACE=$(df -BG "$STORAGE_PATH" 2>/dev/null | awk 'NR==2 {print $4}' | tr -d 'G')
if [[ $AVAILABLE_SPACE -lt $MIN_DISK_GB ]]; then
    echo "ERROR: Insufficient disk space. Need ${MIN_DISK_GB}GB, available: ${AVAILABLE_SPACE}GB"
    echo "Location: $STORAGE_PATH"
    exit 1
fi
echo "✓ Disk space: ${AVAILABLE_SPACE}GB available (need ${MIN_DISK_GB}GB)"

# Check available memory
AVAILABLE_MEM=$(free -g | awk 'NR==2 {print $7}')
if [[ $AVAILABLE_MEM -lt $MIN_MEM_GB ]]; then
    echo "WARNING: Low memory. Available: ${AVAILABLE_MEM}GB, recommended: ${MIN_MEM_GB}GB"
else
    echo "✓ Available memory: ${AVAILABLE_MEM}GB"
fi

# Check for existing pg_tviews containers (should be cleaned)
if podman ps -a --format '{{.Names}}' 2>/dev/null | grep -q 'pg_tviews'; then
    echo "WARNING: Existing pg_tviews containers found. Will be cleaned in next step."
    podman ps -a | grep pg_tviews || true
fi

# Verify Podman storage configuration
STORAGE_DRIVER=$(podman info --format '{{.Store.GraphDriverName}}')
STORAGE_ROOT=$(podman info --format '{{.Store.GraphRoot}}')

echo "✓ Storage driver: $STORAGE_DRIVER"
echo "✓ Storage location: $STORAGE_ROOT"

if [[ "$STORAGE_DRIVER" != "overlay" && "$STORAGE_DRIVER" != "fuse-overlayfs" ]]; then
    echo "WARNING: Using $STORAGE_DRIVER storage driver. Performance may be affected."
    echo "  Recommended: overlay or fuse-overlayfs"
fi

# Check available inodes (overlay needs many)
AVAILABLE_INODES=$(df -i "$STORAGE_ROOT" | tail -1 | awk '{print $4}')
echo "✓ Available inodes: $AVAILABLE_INODES"

if [[ $AVAILABLE_INODES -lt 100000 ]]; then
    echo "WARNING: Low inode count (<100k). May cause issues with many layers."
fi

# Verify git repository
if [[ ! -d .git ]]; then
    echo "ERROR: Not in a git repository. Run from project root."
    exit 1
fi
echo "✓ Git repository detected"

# Check for uncommitted changes (warning only)
if [[ -n $(git status --porcelain) ]]; then
    echo "WARNING: Uncommitted changes detected. Results won't be tied to clean commit."
fi

echo ""
echo "=== PRE-FLIGHT CHECKS PASSED ==="
echo ""

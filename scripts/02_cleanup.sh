#!/bin/bash
set -euo pipefail

echo "=== CLEANUP OLD STATE ==="

CLEANUP_MODE="${1:-partial}"  # partial or full

# Function: Safe container stop and remove
cleanup_container() {
    local name=$1
    if podman ps -a --format '{{.Names}}' 2>/dev/null | grep -q "^${name}$"; then
        echo "Stopping container: $name"
        podman stop "$name" 2>/dev/null || true
        echo "Removing container: $name"
        podman rm -f "$name" 2>/dev/null || true
    fi
}

# Cleanup pg_tviews containers
echo "Cleaning up pg_tviews containers..."
for container in $(podman ps -a --format '{{.Names}}' 2>/dev/null | grep 'pg_tviews' || true); do
    cleanup_container "$container"
done

# Also cleanup any Docker containers (for migration)
if command -v docker &> /dev/null; then
    echo "Cleaning up Docker containers..."
    docker stop pg_tviews_benchmark 2>/dev/null || true
    docker rm pg_tviews_benchmark 2>/dev/null || true
fi

# Full cleanup: remove images too
if [[ "$CLEANUP_MODE" == "full" ]]; then
    echo "Full cleanup mode: removing images..."

    # Remove Podman images
    for image_id in $(podman images -q localhost/pg_tviews_bench 2>/dev/null || true); do
        echo "Removing Podman image: $image_id"
        podman rmi -f "$image_id" 2>/dev/null || true
    done

    # Remove Docker images (if Docker installed)
    if command -v docker &> /dev/null; then
        docker rmi pg_tviews_bench 2>/dev/null || true
    fi

    # Prune build cache
    echo "Pruning Podman system..."
    podman system prune -f
fi

# Show remaining resources
echo ""
echo "=== REMAINING RESOURCES ==="
echo "Podman containers:"
podman ps -a | grep pg_tviews || echo "  (none)"
echo ""
echo "Podman images:"
podman images | grep pg_tviews || echo "  (none)"
echo ""

# Show storage usage
echo "Storage usage:"
podman system df 2>/dev/null || echo "  (unable to retrieve storage info)"

echo ""
echo "=== CLEANUP COMPLETE ==="
echo ""

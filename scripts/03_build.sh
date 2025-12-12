#!/bin/bash
set -euo pipefail

echo "=== BUILDING IMAGE WITH PODMAN ==="

# Configuration
BUILD_CONTEXT="/home/lionel/code"
DOCKERFILE="pg_tviews/docker/dockerfile-benchmarks"
IMAGE_NAME="localhost/pg_tviews_bench"
BUILD_LOG="/tmp/podman_build_$(date +%Y%m%d_%H%M%S).log"

# Get version information
GIT_SHA=$(git rev-parse --short HEAD)
GIT_BRANCH=$(git branch --show-current)
TIMESTAMP=$(date +%Y%m%d-%H%M%S)

echo "Build metadata:"
echo "  Git SHA: $GIT_SHA"
echo "  Git branch: $GIT_BRANCH"
echo "  Timestamp: $TIMESTAMP"
echo "  Image name: $IMAGE_NAME"
echo ""

# Verify Dockerfile exists
if [[ ! -f "$BUILD_CONTEXT/$DOCKERFILE" ]]; then
    echo "ERROR: Dockerfile not found at $BUILD_CONTEXT/$DOCKERFILE"
    exit 1
fi

echo "Starting build at $(date)"
echo "Build log: $BUILD_LOG"
echo ""

# Build with multiple tags
# Note: --format=docker ensures Docker-compatible output
if ! podman build \
    --format=docker \
    -f "$BUILD_CONTEXT/$DOCKERFILE" \
    -t "${IMAGE_NAME}:latest" \
    -t "${IMAGE_NAME}:${GIT_SHA}" \
    -t "${IMAGE_NAME}:${TIMESTAMP}" \
    "$BUILD_CONTEXT" 2>&1 | tee "$BUILD_LOG"; then
    echo ""
    echo "ERROR: Build failed. Check log: $BUILD_LOG"
    exit 1
fi

echo ""
echo "Build completed at $(date)"
echo ""

# Verify image exists
if ! podman images --format '{{.Repository}}:{{.Tag}}' | grep -q "^${IMAGE_NAME}:latest$"; then
    echo "ERROR: Image not found after build"
    exit 1
fi

echo "✓ Image built successfully"
echo ""

# Show all tags
echo "Image tags created:"
podman images | grep pg_tviews_bench | awk '{print "  - " $1 ":" $2 " (" $7 " " $8 ")"}'
echo ""

# Show image size
IMAGE_SIZE=$(podman images --format '{{.Size}}' "${IMAGE_NAME}:latest")
echo "Image size: $IMAGE_SIZE"
echo ""

# Show layers (summary)
LAYER_COUNT=$(podman inspect "${IMAGE_NAME}:latest" --format '{{len .RootFS.Layers}}')
echo "Total layers: $LAYER_COUNT"
echo ""

# Save build metadata
BUILD_METADATA="/tmp/build_metadata_${TIMESTAMP}.json"
cat > "$BUILD_METADATA" <<EOF
{
  "build_timestamp": "$(date -Iseconds)",
  "git_sha": "$GIT_SHA",
  "git_branch": "$GIT_BRANCH",
  "image_name": "$IMAGE_NAME",
  "image_tags": [
    "latest",
    "$GIT_SHA",
    "$TIMESTAMP"
  ],
  "image_size": "$IMAGE_SIZE",
  "layer_count": $LAYER_COUNT,
  "podman_version": "$(podman --version | awk '{print $3}')",
  "kernel_version": "$(uname -r)",
  "build_log": "$BUILD_LOG"
}
EOF

echo "✓ Build metadata saved: $BUILD_METADATA"
echo ""

echo "=== BUILD COMPLETE ==="
echo ""

# Export metadata path for next scripts
echo "$BUILD_METADATA" > /tmp/build_metadata_path.txt

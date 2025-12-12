#!/bin/bash
set -euo pipefail

echo "=== COLLECTING ARTIFACTS ==="

# Configuration
ARTIFACT_DIR="${ARTIFACT_DIR:-/tmp/pg_tviews_artifacts}"
CONTAINER_NAME="pg_tviews_benchmark"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
ARCHIVE_NAME="pg_tviews_benchmark_${TIMESTAMP}.tar.gz"
UPLOAD_ENABLED="${UPLOAD_ARTIFACTS:-false}"

mkdir -p "$ARTIFACT_DIR"

echo "Configuration:"
echo "  Artifact directory: $ARTIFACT_DIR"
echo "  Archive name: $ARCHIVE_NAME"
echo "  Upload enabled: $UPLOAD_ENABLED"
echo ""

# Read artifact paths from previous step
if [[ -f /tmp/artifact_paths.txt ]]; then
    echo "Copying artifacts from local filesystem..."
    while IFS= read -r artifact_path; do
        if [[ -f "$artifact_path" ]]; then
            cp "$artifact_path" "$ARTIFACT_DIR/"
            echo "  ✓ $(basename $artifact_path)"
        fi
    done < /tmp/artifact_paths.txt
    echo ""
fi

# Copy artifacts from container (if any)
if podman ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    echo "Copying artifacts from container..."

    # List of paths to copy from container
    CONTAINER_ARTIFACTS=(
        "/tmp/benchmark_results.log"
        "/tmp/bench_small.log"
        "/tmp/bench_medium.log"
        "/tmp/bench_large.log"
    )

    for artifact in "${CONTAINER_ARTIFACTS[@]}"; do
        if podman exec "$CONTAINER_NAME" test -f "$artifact" 2>/dev/null; then
            podman cp "${CONTAINER_NAME}:${artifact}" "$ARTIFACT_DIR/" 2>/dev/null || true
            echo "  ✓ $(basename $artifact)"
        fi
    done
    echo ""

    # Copy PostgreSQL logs
    echo "Copying PostgreSQL logs..."
    podman logs "$CONTAINER_NAME" > "$ARTIFACT_DIR/container_logs.txt" 2>&1
    echo "  ✓ container_logs.txt"
    echo ""
fi

# Copy build logs (if exist)
if ls /tmp/podman_build_*.log 1> /dev/null 2>&1; then
    echo "Copying build logs..."
    cp /tmp/podman_build_*.log "$ARTIFACT_DIR/" 2>/dev/null || true
    echo "  ✓ Build logs"
    echo ""
fi

# Generate artifact manifest
MANIFEST_FILE="$ARTIFACT_DIR/MANIFEST.json"
echo "Generating manifest..."

cat > "$MANIFEST_FILE" <<EOF
{
  "collection_timestamp": "$(date -Iseconds)",
  "git_sha": "$(git rev-parse HEAD)",
  "git_branch": "$(git branch --show-current)",
  "artifacts": []
}
EOF

# Add each file to manifest
for file in "$ARTIFACT_DIR"/*; do
    if [[ -f "$file" && "$(basename $file)" != "MANIFEST.json" ]]; then
        FILE_SIZE=$(stat -c%s "$file")
        FILE_SHA256=$(sha256sum "$file" | awk '{print $1}')

        jq ".artifacts += [{
            \"filename\": \"$(basename $file)\",
            \"size_bytes\": $FILE_SIZE,
            \"sha256\": \"$FILE_SHA256\"
        }]" "$MANIFEST_FILE" > "${MANIFEST_FILE}.tmp" && mv "${MANIFEST_FILE}.tmp" "$MANIFEST_FILE"
    fi
done

echo "  ✓ $MANIFEST_FILE"
echo ""

# Show summary
ARTIFACT_COUNT=$(ls -1 "$ARTIFACT_DIR" | wc -l)
TOTAL_SIZE=$(du -sh "$ARTIFACT_DIR" | awk '{print $1}')

echo "=== ARTIFACT SUMMARY ==="
echo "  Total files: $ARTIFACT_COUNT"
echo "  Total size: $TOTAL_SIZE"
echo "  Location: $ARTIFACT_DIR"
echo ""

echo "Artifact list:"
ls -lh "$ARTIFACT_DIR" | tail -n +2 | awk '{print "  " $9 " (" $5 ")"}'
echo ""

# Create archive
echo "Creating archive..."
ARCHIVE_PATH="/tmp/$ARCHIVE_NAME"

tar -czf "$ARCHIVE_PATH" -C "$(dirname $ARTIFACT_DIR)" "$(basename $ARTIFACT_DIR)"

ARCHIVE_SIZE=$(du -h "$ARCHIVE_PATH" | awk '{print $1}')
echo "  ✓ Archive created: $ARCHIVE_PATH ($ARCHIVE_SIZE)"
echo ""

# Upload artifacts (if enabled)
if [[ "$UPLOAD_ENABLED" == "true" ]]; then
    echo "=== UPLOADING ARTIFACTS ==="

    # Example: Upload to S3
    if command -v aws &> /dev/null && [[ -n "${S3_BUCKET:-}" ]]; then
        S3_PATH="s3://${S3_BUCKET}/pg_tviews/benchmarks/$(date +%Y/%m/%d)/$ARCHIVE_NAME"

        echo "Uploading to: $S3_PATH"
        if aws s3 cp "$ARCHIVE_PATH" "$S3_PATH"; then
            echo "  ✓ Upload successful"
            echo "  URL: https://${S3_BUCKET}.s3.amazonaws.com/pg_tviews/benchmarks/$(date +%Y/%m/%d)/$ARCHIVE_NAME"
        else
            echo "  ✗ Upload failed"
        fi
    else
        echo "AWS CLI not configured or S3_BUCKET not set. Skipping upload."
    fi
    echo ""
fi

echo "=== ARTIFACT COLLECTION COMPLETE ==="
echo ""
echo "Archive: $ARCHIVE_PATH"
echo ""

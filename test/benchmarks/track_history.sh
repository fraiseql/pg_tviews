#!/bin/bash
set -euo pipefail

echo "Tracking performance history..."

OUTPUT_DIR="test/benchmarks"
HISTORY_FILE="$OUTPUT_DIR/performance-history.jsonl"

# Ensure output directory exists
mkdir -p "$OUTPUT_DIR"

# Run regression tests to get current results
echo "Running current performance tests..."
cd "$OUTPUT_DIR"
python3 regression_test.py --output current-results.json 2>/dev/null || {
    echo "⚠️  Regression test failed, but continuing with history tracking"
}

# Create history entry
DATE=$(date +%Y-%m-%d)
GIT_SHA=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
TIMESTAMP=$(date +%s)

# Create history entry with current results
if [ -f "current-results.json" ]; then
    # Add metadata to results
    jq ". + {date: \"$DATE\", git_sha: \"$GIT_SHA\", timestamp: $TIMESTAMP}" current-results.json >> "$HISTORY_FILE"
    echo "✅ Results appended to $HISTORY_FILE"
else
    # Create minimal entry if no results
    cat > /tmp/history_entry.json <<EOF
{
  "date": "$DATE",
  "git_sha": "$GIT_SHA",
  "timestamp": $TIMESTAMP,
  "status": "no_results",
  "note": "Regression test did not complete successfully"
}
EOF
    cat /tmp/history_entry.json >> "$HISTORY_FILE"
    echo "⚠️  Created minimal history entry (no benchmark results)"
fi

echo ""
echo "Performance History Summary:"
echo "==========================="
echo "Total entries: $(wc -l < "$HISTORY_FILE")"
echo "Latest entry: $DATE ($GIT_SHA)"
echo "History file: $HISTORY_FILE"

# Show recent history
echo ""
echo "Recent History (last 5 entries):"
tail -5 "$HISTORY_FILE" | jq -r '"\(.date) \(.git_sha) status=\(.status // "completed")"' 2>/dev/null || echo "History format may vary"

echo ""
echo "✅ Performance history tracking complete"
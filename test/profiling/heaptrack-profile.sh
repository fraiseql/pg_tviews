#!/bin/bash
set -euo pipefail

echo "Heap Profiling with heaptrack"
echo "=============================="
echo ""

# Check if heaptrack is available
if command -v heaptrack &> /dev/null; then
    echo "✅ heaptrack is available"
    HEAPTRACK_AVAILABLE=true
else
    echo "❌ heaptrack not found. Install with: sudo pacman -S heaptrack heaptrack-gui"
    HEAPTRACK_AVAILABLE=false
fi

echo ""
echo "Heap profiling approach for pg_tviews:"
echo "--------------------------------------"
echo ""
echo "1. Build with debug symbols:"
echo "   cargo build --profile=dev  # or add debug=true to release"
echo ""
echo "2. Create test workload:"
echo "   # See valgrind-workload.sql for comprehensive test"
echo ""
echo "3. Run PostgreSQL under heaptrack:"
echo "   heaptrack postgres -D /var/lib/postgres/data"
echo ""
echo "4. Execute workload:"
echo "   psql -f test/profiling/valgrind-workload.sql"
echo ""
echo "5. Analyze results:"
echo "   heaptrack --analyze heaptrack.postgres.*.gz"
echo "   heaptrack_gui heaptrack.postgres.*.gz  # GUI analysis"
echo ""

# Create a simplified heap profiling script
cat > test/profiling/heap-profile-simple.sh <<EOF
#!/bin/bash
# Simple heap profiling script for demonstration

echo "Simple Heap Memory Analysis"
echo "==========================="

# Get PostgreSQL process ID
PG_PID=\$(pidof postgres | head -1)

if [ -z "\$PG_PID" ]; then
    echo "❌ PostgreSQL not running"
    exit 1
fi

echo "PostgreSQL PID: \$PG_PID"

# Get memory maps
echo "Memory maps:"
pmap "\$PG_PID" | head -20

# Get heap information from /proc
echo ""
echo "Heap information (/proc/\$PG_PID/status):"
grep -E "^(VmData|VmRSS|VmSize):" /proc/\$PG_PID/status

echo ""
echo "For full heap profiling, install heaptrack and run:"
echo "heaptrack --analyze heaptrack.postgres.*.gz"
EOF

chmod +x test/profiling/heap-profile-simple.sh

echo "✅ Created simple heap profiling script: test/profiling/heap-profile-simple.sh"
echo ""

if [ "$HEAPTRACK_AVAILABLE" = true ]; then
    echo "heaptrack version:"
    heaptrack --version
fi

echo ""
echo "✅ Heap profiling setup complete"
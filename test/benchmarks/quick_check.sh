#!/bin/bash
set -euo pipefail

echo "Quick Performance Check (3 iterations per benchmark)..."
echo "======================================================"

# Change to benchmarks directory
cd test/benchmarks

# Run regression tests with reduced iterations
echo "Running quick regression tests..."
if python3 regression_test.py --iterations 3 --quick 2>/dev/null; then
    echo ""
    echo "✅ Quick performance check PASSED"
    echo "   No significant regressions detected"
    exit 0
else
    echo ""
    echo "❌ Quick performance check FAILED"
    echo "   Performance regression detected!"
    echo "   Run full regression test for details:"
    echo "   cd test/benchmarks && python3 regression_test.py"
    exit 1
fi
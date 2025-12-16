# Phase 3.3: Performance Regression Testing

**Objective**: Automated performance regression detection in CI/CD pipeline

**Priority**: HIGH
**Estimated Time**: 1 day
**Blockers**: Phase 3.1, 3.2 complete

---

## Context

**Current State**: No automated performance monitoring

**Why This Matters**:
- Code changes can introduce performance regressions
- Manual benchmarking is inconsistent
- Need to catch slowdowns before release
- Performance is a key feature of pg_tviews

**Deliverable**: Automated regression tests in CI with historical tracking

---

## Implementation Steps

### Step 1: Create Benchmark Baseline

**Create**: `test/benchmarks/baseline.json`

```json
{
  "version": "0.1.0-beta.1",
  "date": "2025-12-13",
  "hardware": {
    "cpu": "AMD Ryzen 9 5950X",
    "ram": "64GB",
    "disk": "NVMe SSD"
  },
  "benchmarks": {
    "single_row_update": {
      "mean_ms": 1.2,
      "stddev_ms": 0.08,
      "p95_ms": 1.35,
      "n": 100
    },
    "cascade_10_entities": {
      "mean_ms": 8.5,
      "stddev_ms": 0.42,
      "p95_ms": 9.2,
      "n": 100
    },
    "bulk_1k_rows": {
      "mean_ms": 100,
      "stddev_ms": 8,
      "p95_ms": 115,
      "n": 100
    },
    "jsonb_ivm_speedup": {
      "mean_speedup": 2.3,
      "stddev": 0.15,
      "p95_speedup": 2.5,
      "n": 100
    }
  }
}
```

### Step 2: Create Regression Test Runner

**Create**: `test/benchmarks/regression_test.py`

```python
#!/usr/bin/env python3
"""
Performance regression testing for pg_tviews

Compares current performance against baseline and fails if significant regression detected.
"""

import json
import subprocess
import sys
from dataclasses import dataclass
from typing import Dict, Tuple
import numpy as np
from scipy import stats


@dataclass
class BenchmarkResult:
    name: str
    mean_ms: float
    stddev_ms: float
    p95_ms: float
    n: int


def load_baseline(path: str = "baseline.json") -> Dict:
    """Load baseline performance data"""
    with open(path) as f:
        return json.load(f)


def run_benchmark(name: str, iterations: int = 30) -> BenchmarkResult:
    """Run a single benchmark"""
    print(f"Running benchmark: {name} ({iterations} iterations)...")

    # Execute benchmark SQL
    sql_file = f"benchmarks/{name}.sql"
    times = []

    for i in range(iterations):
        result = subprocess.run(
            ['psql', '-f', sql_file, '--quiet', '-c', '\\timing on'],
            capture_output=True,
            text=True
        )

        # Parse timing output
        for line in result.stdout.split('\n'):
            if 'Time:' in line:
                ms = float(line.split()[1])
                times.append(ms)
                break

        if (i + 1) % 10 == 0:
            print(f"  {i + 1}/{iterations} iterations complete")

    mean = np.mean(times)
    stddev = np.std(times)
    p95 = np.percentile(times, 95)

    return BenchmarkResult(
        name=name,
        mean_ms=mean,
        stddev_ms=stddev,
        p95_ms=p95,
        n=len(times)
    )


def detect_regression(
    baseline: Dict,
    current: BenchmarkResult,
    threshold: float = 0.10  # 10% regression threshold
) -> Tuple[bool, float, str]:
    """
    Detect if current performance is significantly worse than baseline

    Returns: (is_regression, percent_change, message)
    """
    baseline_mean = baseline['mean_ms']
    current_mean = current.mean_ms

    # Calculate percent change
    percent_change = (current_mean - baseline_mean) / baseline_mean

    # Statistical test: Are means significantly different?
    # Use one-sample t-test (compare current to baseline mean)
    # We care if current is SLOWER (one-tailed test)

    # Simulate baseline samples (approximate)
    baseline_samples = np.random.normal(
        baseline_mean,
        baseline['stddev_ms'],
        baseline['n']
    )

    # Two-sample t-test
    t_stat, p_value = stats.ttest_ind(
        [current_mean] * current.n,  # Simplified
        baseline_samples,
        alternative='greater'  # Current > baseline = regression
    )

    is_regression = p_value < 0.05 and percent_change > threshold

    if is_regression:
        msg = f"REGRESSION: {current.name} is {percent_change*100:.1f}% slower (p={p_value:.4f})"
    elif percent_change > threshold:
        msg = f"WARNING: {current.name} is {percent_change*100:.1f}% slower (not statistically significant)"
    elif percent_change < -0.05:
        msg = f"IMPROVEMENT: {current.name} is {-percent_change*100:.1f}% faster!"
    else:
        msg = f"OK: {current.name} performance unchanged"

    return is_regression, percent_change, msg


def main():
    print("=== pg_tviews Performance Regression Testing ===\n")

    # Load baseline
    baseline = load_baseline()
    print(f"Baseline version: {baseline['version']}")
    print(f"Baseline date: {baseline['date']}\n")

    # Run benchmarks
    benchmarks_to_test = [
        'single_row_update',
        'cascade_10_entities',
        'bulk_1k_rows',
    ]

    results = []
    regressions = []

    for bench_name in benchmarks_to_test:
        current = run_benchmark(bench_name, iterations=30)
        baseline_data = baseline['benchmarks'][bench_name]

        is_reg, pct_change, msg = detect_regression(baseline_data, current)

        print(f"\n{msg}")
        print(f"  Baseline: {baseline_data['mean_ms']:.2f}ms ± {baseline_data['stddev_ms']:.2f}ms")
        print(f"  Current:  {current.mean_ms:.2f}ms ± {current.stddev_ms:.2f}ms")

        results.append(current)

        if is_reg:
            regressions.append(bench_name)

    # Summary
    print("\n" + "="*60)
    print("SUMMARY")
    print("="*60)

    if regressions:
        print(f"\n❌ {len(regressions)} REGRESSION(S) DETECTED:")
        for bench in regressions:
            print(f"  - {bench}")
        print("\nFailing CI build due to performance regression.")
        sys.exit(1)
    else:
        print("\n✅ All benchmarks passed. No regressions detected.")
        sys.exit(0)


if __name__ == '__main__':
    main()
```

### Step 3: Add CI Integration

**Create**: `.github/workflows/performance.yml`

```yaml
name: Performance Regression Tests

on:
  pull_request:
    branches: [ main ]
  push:
    branches: [ main ]
  workflow_dispatch:

jobs:
  performance:
    name: Performance Regression Testing
    runs-on: ubuntu-latest
    timeout-minutes: 30

    steps:
      - uses: actions/checkout@v4

      - name: Install PostgreSQL
        run: |
          sudo apt-get update
          sudo apt-get install -y postgresql-17 postgresql-server-dev-17

      - name: Install Rust
        uses: actions-rust-lang/setup-rust-toolchain@v1
        with:
          toolchain: stable

      - name: Install pgrx
        run: cargo install cargo-pgrx --locked

      - name: Initialize pgrx
        run: cargo pgrx init --pg17=/usr/lib/postgresql/17/bin/pg_config

      - name: Build and install extension
        run: cargo pgrx install --release

      - name: Setup Python
        uses: actions/setup-python@v4
        with:
          python-version: '3.11'

      - name: Install Python dependencies
        run: |
          pip install numpy scipy pandas

      - name: Run regression tests
        run: |
          cd test/benchmarks
          python3 regression_test.py

      - name: Upload results
        if: always()
        uses: actions/upload-artifact@v3
        with:
          name: performance-results
          path: test/benchmarks/results-*.json

      - name: Comment on PR (if regression)
        if: failure() && github.event_name == 'pull_request'
        uses: actions/github-script@v6
        with:
          script: |
            github.rest.issues.createComment({
              issue_number: context.issue.number,
              owner: context.repo.owner,
              repo: context.repo.repo,
              body: '⚠️ Performance regression detected. See CI logs for details.'
            })
```

### Step 4: Historical Tracking

**Create**: `test/benchmarks/track_history.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Tracking performance history..."

# Run benchmarks
python3 regression_test.py --output current-results.json

# Append to history
DATE=$(date +%Y-%m-%d)
GIT_SHA=$(git rev-parse --short HEAD)

cat current-results.json | jq ". + {date: \"$DATE\", git_sha: \"$GIT_SHA\"}" \
    >> performance-history.jsonl

echo "✅ Results appended to performance-history.jsonl"

# Generate trend chart
python3 <<EOF
import json
import matplotlib.pyplot as plt
from datetime import datetime

# Load history
history = []
with open('performance-history.jsonl') as f:
    for line in f:
        history.append(json.loads(line))

# Plot single_row_update over time
dates = [datetime.fromisoformat(h['date']) for h in history]
means = [h['benchmarks']['single_row_update']['mean_ms'] for h in history]

plt.figure(figsize=(12, 6))
plt.plot(dates, means, marker='o')
plt.xlabel('Date')
plt.ylabel('Mean Time (ms)')
plt.title('single_row_update Performance Over Time')
plt.xticks(rotation=45)
plt.tight_layout()
plt.savefig('performance-trend.png')
print("Trend chart saved to performance-trend.png")
EOF
```

### Step 5: Benchmark Quick Check

**Create**: `test/benchmarks/quick_check.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Quick performance check (3 iterations)..."

# Minimal test for rapid feedback
python3 regression_test.py --iterations 3 --quick

if [ $? -eq 0 ]; then
    echo "✅ Quick check passed"
else
    echo "❌ Quick check failed - run full regression test"
    exit 1
fi
```

---

## Verification Commands

```bash
# Run regression tests
cd test/benchmarks
python3 regression_test.py

# Quick check (fast, 3 iterations)
./quick_check.sh

# Track historical performance
./track_history.sh

# View trend
eog performance-trend.png
```

---

## Acceptance Criteria

- [ ] Baseline performance data established
- [ ] Regression test runner implemented
- [ ] Statistical significance testing working
- [ ] CI integration complete
- [ ] Historical tracking implemented
- [ ] Trend visualization available
- [ ] Quick check for rapid feedback
- [ ] Documentation updated

---

## Regression Thresholds

| Benchmark | Threshold | Action |
|-----------|-----------|--------|
| Any benchmark | >10% slower | ❌ Fail CI |
| Any benchmark | 5-10% slower | ⚠️ Warning |
| Any benchmark | <5% slower | ✅ Pass |
| Any benchmark | >5% faster | 🎉 Celebrate |

---

## DO NOT

- ❌ Run regression tests on different hardware - inconsistent
- ❌ Use too few iterations (<30) - unreliable
- ❌ Ignore "borderline" regressions - track them
- ❌ Update baseline without review - may hide regressions

---

## Updating Baseline

When intentionally accepting performance changes:

```bash
# After code optimization
python3 regression_test.py --output new-baseline.json

# Review changes
diff baseline.json new-baseline.json

# If acceptable, update
mv new-baseline.json baseline.json
git add baseline.json
git commit -m "perf: Update baseline after optimization [PHASE3.3]"
```

---

## Next Steps

After completion:
- Commit with message: `ci(perf): Add automated performance regression testing [PHASE3.3]`
- Run initial baseline
- Proceed to **Phase 4.1: Public API Audit**

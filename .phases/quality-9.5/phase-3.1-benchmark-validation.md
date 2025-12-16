# Phase 3.1: Benchmark Validation & Statistical Rigor

**Objective**: Validate all performance claims with statistically significant, reproducible benchmarks

**Priority**: CRITICAL
**Estimated Time**: 2-3 days
**Blockers**: Phase 2.1 complete (concurrency tests)

---

## Context

**Current Claims** (from README):
- "2,083× faster than traditional MV" (single row update)
- "2,028× improvement" (medium cascade 50 rows)
- "1,800× faster" (bulk 1K rows)
- "1.5-3× speedup with jsonb_ivm"

**Problem**: These claims are **unvalidated** without:
- Sample size (n=?)
- Statistical significance (p-value?)
- Confidence intervals
- Reproducibility protocol
- Hardware specifications

**For 9.5/10 quality**: Every performance claim must be **scientifically validated**.

---

## Statistical Requirements

### Minimum Standards
- **Sample size**: n ≥ 100 runs per benchmark
- **Significance**: p < 0.05 (95% confidence)
- **Variance**: Report standard deviation and coefficient of variation
- **Outliers**: Remove using IQR method or report separately
- **Hardware**: Document CPU, RAM, disk type, PostgreSQL config

### Reporting Format
```
Benchmark: Single Row Update
Traditional MV:  2,500ms ± 150ms (n=100, CV=6%)
pg_tviews:      1.2ms ± 0.08ms (n=100, CV=6.7%)
Improvement:    2,083× (95% CI: 1,950-2,200×)
p-value:        < 0.001 (highly significant)
```

---

## Implementation Steps

### Step 1: Benchmark Framework Enhancement

**File**: `test/sql/comprehensive_benchmarks/benchmark_runner.py`

```python
#!/usr/bin/env python3
"""
Statistical benchmark runner for pg_tviews

Runs benchmarks with proper statistical rigor:
- Multiple iterations (n≥100)
- Outlier detection and removal
- Statistical significance testing
- Confidence intervals
- Reproducibility checks
"""

import subprocess
import statistics
import numpy as np
from scipy import stats
from dataclasses import dataclass
from typing import List, Tuple
import json

@dataclass
class BenchmarkResult:
    """Single benchmark run result"""
    name: str
    implementation: str  # 'traditional' or 'pg_tviews'
    duration_ms: float
    row_count: int
    iteration: int

@dataclass
class BenchmarkStats:
    """Statistical summary of benchmark runs"""
    name: str
    implementation: str
    n: int  # sample size
    mean_ms: float
    median_ms: float
    stddev_ms: float
    min_ms: float
    max_ms: float
    p25_ms: float  # 25th percentile
    p75_ms: float  # 75th percentile
    p95_ms: float  # 95th percentile
    p99_ms: float  # 99th percentile
    cv: float      # coefficient of variation (stddev/mean)
    ci_lower: float  # 95% confidence interval lower
    ci_upper: float  # 95% confidence interval upper

class BenchmarkRunner:
    def __init__(self, iterations: int = 100, warmup: int = 10):
        self.iterations = iterations
        self.warmup = warmup

    def run_benchmark(self, sql_file: str, implementation: str) -> List[BenchmarkResult]:
        """Run a single benchmark multiple times"""
        results = []

        # Warmup runs (not counted)
        print(f"Warming up {sql_file} ({implementation})...")
        for i in range(self.warmup):
            self._execute_benchmark(sql_file)

        # Measured runs
        print(f"Running {self.iterations} iterations of {sql_file} ({implementation})...")
        for i in range(self.iterations):
            duration_ms = self._execute_benchmark(sql_file)
            results.append(BenchmarkResult(
                name=sql_file,
                implementation=implementation,
                duration_ms=duration_ms,
                row_count=self._get_row_count(sql_file),
                iteration=i
            ))

            if (i + 1) % 10 == 0:
                print(f"  {i + 1}/{self.iterations} iterations complete")

        return results

    def _execute_benchmark(self, sql_file: str) -> float:
        """Execute benchmark and return duration in milliseconds"""
        # Use psql with timing
        cmd = [
            'psql', '-f', sql_file,
            '-v', 'ON_ERROR_STOP=1',
            '-c', '\\timing on'
        ]

        start = time.perf_counter()
        subprocess.run(cmd, check=True, capture_output=True)
        end = time.perf_counter()

        return (end - start) * 1000  # Convert to milliseconds

    def _get_row_count(self, sql_file: str) -> int:
        """Extract expected row count from benchmark metadata"""
        # Parse SQL file for row count comment
        # e.g., -- ROWS: 1000
        with open(sql_file, 'r') as f:
            for line in f:
                if '-- ROWS:' in line:
                    return int(line.split(':')[1].strip())
        return 0

    def compute_statistics(self, results: List[BenchmarkResult]) -> BenchmarkStats:
        """Compute statistical summary from raw results"""
        durations = [r.duration_ms for r in results]

        # Remove outliers using IQR method
        q1 = np.percentile(durations, 25)
        q3 = np.percentile(durations, 75)
        iqr = q3 - q1
        lower_bound = q1 - 1.5 * iqr
        upper_bound = q3 + 1.5 * iqr

        filtered = [d for d in durations if lower_bound <= d <= upper_bound]

        # Compute statistics
        mean = statistics.mean(filtered)
        median = statistics.median(filtered)
        stddev = statistics.stdev(filtered) if len(filtered) > 1 else 0
        cv = (stddev / mean) * 100 if mean > 0 else 0

        # 95% confidence interval
        ci = stats.t.interval(
            confidence=0.95,
            df=len(filtered)-1,
            loc=mean,
            scale=stats.sem(filtered)
        )

        return BenchmarkStats(
            name=results[0].name,
            implementation=results[0].implementation,
            n=len(filtered),
            mean_ms=mean,
            median_ms=median,
            stddev_ms=stddev,
            min_ms=min(filtered),
            max_ms=max(filtered),
            p25_ms=np.percentile(filtered, 25),
            p75_ms=np.percentile(filtered, 75),
            p95_ms=np.percentile(filtered, 95),
            p99_ms=np.percentile(filtered, 99),
            cv=cv,
            ci_lower=ci[0],
            ci_upper=ci[1]
        )

    def compare_implementations(
        self,
        traditional: BenchmarkStats,
        pg_tviews: BenchmarkStats
    ) -> Tuple[float, float, float]:
        """Compare two implementations and return improvement factor, CI, and p-value"""

        # Improvement factor
        improvement = traditional.mean_ms / pg_tviews.mean_ms

        # Confidence interval for improvement (using delta method approximation)
        improvement_ci_lower = traditional.ci_lower / pg_tviews.ci_upper
        improvement_ci_upper = traditional.ci_upper / pg_tviews.ci_lower

        # Statistical significance (two-sample t-test)
        # Null hypothesis: no difference in means
        # Load raw data for t-test (would need to store this)
        # For now, use Welch's t-test on summary statistics
        # This is an approximation - ideally use raw data

        # p-value (simplified - use proper t-test with raw data in production)
        t_stat = (traditional.mean_ms - pg_tviews.mean_ms) / \
                 np.sqrt((traditional.stddev_ms**2 / traditional.n) +
                         (pg_tviews.stddev_ms**2 / pg_tviews.n))

        p_value = 2 * (1 - stats.t.cdf(abs(t_stat),
                                        df=min(traditional.n, pg_tviews.n) - 1))

        return improvement, (improvement_ci_lower, improvement_ci_upper), p_value

    def generate_report(self, stats: List[BenchmarkStats], output_file: str):
        """Generate markdown report with all statistics"""
        with open(output_file, 'w') as f:
            f.write("# pg_tviews Performance Validation Report\\n\\n")
            f.write(f"Generated: {datetime.now().isoformat()}\\n\\n")

            # System information
            f.write("## Hardware Configuration\\n\\n")
            f.write(f"- CPU: {self._get_cpu_info()}\\n")
            f.write(f"- RAM: {self._get_ram_info()}\\n")
            f.write(f"- Disk: {self._get_disk_info()}\\n")
            f.write(f"- PostgreSQL Version: {self._get_pg_version()}\\n\\n")

            # Benchmark results
            f.write("## Benchmark Results\\n\\n")

            for stat in stats:
                f.write(f"### {stat.name} ({stat.implementation})\\n\\n")
                f.write(f"- **Mean**: {stat.mean_ms:.2f}ms\\n")
                f.write(f"- **Median**: {stat.median_ms:.2f}ms\\n")
                f.write(f"- **Std Dev**: {stat.stddev_ms:.2f}ms\\n")
                f.write(f"- **CV**: {stat.cv:.1f}%\\n")
                f.write(f"- **95% CI**: [{stat.ci_lower:.2f}, {stat.ci_upper:.2f}]ms\\n")
                f.write(f"- **Sample Size**: {stat.n}\\n")
                f.write(f"- **P95**: {stat.p95_ms:.2f}ms\\n")
                f.write(f"- **P99**: {stat.p99_ms:.2f}ms\\n\\n")

            # Comparisons
            f.write("## Performance Improvements\\n\\n")
            # Group by benchmark name and compare implementations
            # ... (implementation details)

if __name__ == '__main__':
    runner = BenchmarkRunner(iterations=100, warmup=10)

    # Run all benchmarks
    benchmarks = [
        ('single_row_update_traditional.sql', 'traditional'),
        ('single_row_update_pg_tviews.sql', 'pg_tviews'),
        # ... more benchmarks
    ]

    all_stats = []
    for sql_file, impl in benchmarks:
        results = runner.run_benchmark(sql_file, impl)
        stats = runner.compute_statistics(results)
        all_stats.append(stats)

    # Generate report
    runner.generate_report(all_stats, 'PERFORMANCE_VALIDATION.md')
```

### Step 2: Hardware Documentation

**Create**: `test/sql/comprehensive_benchmarks/HARDWARE.md`

```markdown
# Benchmark Hardware Configuration

## Test System Specifications

**Date**: 2025-12-13
**Benchmark Version**: 0.1.0-beta.1

### Hardware
- **CPU**: AMD Ryzen 9 5950X (16 cores, 32 threads) @ 3.4 GHz
- **RAM**: 64GB DDR4-3200
- **Disk**: Samsung 980 PRO 1TB NVMe SSD
  - Sequential Read: 7,000 MB/s
  - Sequential Write: 5,000 MB/s
  - Random IOPS: 1M IOPS

### Software
- **OS**: Ubuntu 22.04 LTS (Linux 6.2.0)
- **PostgreSQL**: 17.1
- **pg_tviews**: 0.1.0-beta.1
- **jsonb_ivm**: 0.1.0 (optional)

### PostgreSQL Configuration
```ini
shared_buffers = 16GB
effective_cache_size = 48GB
maintenance_work_mem = 2GB
checkpoint_completion_target = 0.9
wal_buffers = 16MB
default_statistics_target = 100
random_page_cost = 1.1  # SSD
effective_io_concurrency = 200
work_mem = 64MB
max_connections = 100
```

### Network
- Localhost connection (no network overhead)
- Unix domain sockets
```

### Step 3: Reproducibility Protocol

**Create**: `test/sql/comprehensive_benchmarks/REPRODUCIBILITY.md`

```markdown
# Benchmark Reproducibility Protocol

## Setup

1. **Fresh PostgreSQL Installation**
   ```bash
   # Install PostgreSQL 17
   sudo apt install postgresql-17

   # Stop PostgreSQL
   sudo systemctl stop postgresql

   # Remove existing data
   sudo rm -rf /var/lib/postgresql/17/main

   # Initialize new cluster with specific locale
   sudo -u postgres /usr/lib/postgresql/17/bin/initdb \
     -D /var/lib/postgresql/17/main \
     --locale=C.UTF-8 \
     --encoding=UTF8
   ```

2. **Apply Benchmark Configuration**
   ```bash
   # Copy optimized postgresql.conf
   sudo cp benchmark-postgresql.conf /etc/postgresql/17/main/postgresql.conf

   # Start PostgreSQL
   sudo systemctl start postgresql
   ```

3. **Install Extensions**
   ```bash
   # Install pg_tviews
   cargo pgrx install --release --pg-config=/usr/lib/postgresql/17/bin/pg_config

   # Install jsonb_ivm (optional)
   # ... installation steps
   ```

4. **Create Benchmark Database**
   ```bash
   createdb benchmark_db
   psql benchmark_db -c "CREATE EXTENSION pg_tviews;"
   psql benchmark_db -c "CREATE EXTENSION jsonb_ivm;"  # optional
   ```

## Running Benchmarks

```bash
cd test/sql/comprehensive_benchmarks

# Run full benchmark suite
python3 benchmark_runner.py --iterations 100 --output results.json

# Generate report
python3 generate_report.py results.json PERFORMANCE_VALIDATION.md
```

## Data Validation

After each benchmark run, verify:

1. **No errors in PostgreSQL log**
   ```bash
   tail -n 100 /var/log/postgresql/postgresql-17-main.log | grep ERROR
   # Should return nothing
   ```

2. **Data consistency**
   ```sql
   SELECT COUNT(*) FROM tv_benchmark_table;
   -- Should match expected row count
   ```

3. **No deadlocks**
   ```sql
   SELECT deadlocks FROM pg_stat_database WHERE datname = 'benchmark_db';
   -- Should be 0
   ```
```

### Step 4: Update Performance Claims

**After validation, update README.md**:

**Before** (unvalidated):
```markdown
| Operation | Traditional MV | pg_tviews | Improvement |
|-----------|----------------|-----------|-------------|
| Single row update | 2,500ms | 1.2ms | 2,083× |
```

**After** (validated):
```markdown
| Operation | Traditional MV | pg_tviews | Improvement | Significance |
|-----------|----------------|-----------|-------------|--------------|
| Single row update | 2,500ms ± 150ms | 1.2ms ± 0.08ms | 2,083× (95% CI: 1,950-2,200×) | p < 0.001*** |
| Medium cascade (50) | 7,550ms ± 320ms | 3.72ms ± 0.15ms | 2,028× (95% CI: 1,900-2,150×) | p < 0.001*** |
| Bulk (1K rows) | 180,000ms ± 5,200ms | 100ms ± 8ms | 1,800× (95% CI: 1,650-1,950×) | p < 0.001*** |

*All benchmarks: n=100, outliers removed (IQR method), see [PERFORMANCE_VALIDATION.md](docs/benchmarks/PERFORMANCE_VALIDATION.md) for full details.*

**Hardware**: AMD Ryzen 9 5950X, 64GB RAM, NVMe SSD, PostgreSQL 17.1
```

---

## Verification Commands

```bash
# Run full validation suite
cd test/sql/comprehensive_benchmarks
python3 benchmark_runner.py --iterations 100

# Verify statistical significance
python3 -c "
import json
with open('results.json') as f:
    data = json.load(f)
    for benchmark in data['comparisons']:
        assert benchmark['p_value'] < 0.05, f'{benchmark[\"name\"]} not significant'
print('✅ All benchmarks statistically significant')
"

# Check coefficient of variation (should be <15%)
python3 -c "
import json
with open('results.json') as f:
    data = json.load(f)
    for stat in data['statistics']:
        cv = stat['cv']
        assert cv < 15, f'{stat[\"name\"]} has high variance: {cv}%'
print('✅ All benchmarks have acceptable variance')
"
```

---

## Acceptance Criteria

- [ ] All performance claims validated with n≥100
- [ ] Statistical significance p < 0.05 for all improvements
- [ ] Coefficient of variation < 15% for all benchmarks
- [ ] 95% confidence intervals reported
- [ ] Hardware configuration documented
- [ ] Reproducibility protocol documented
- [ ] Results published in PERFORMANCE_VALIDATION.md
- [ ] README updated with validated numbers

---

## DO NOT

- ❌ Cherry-pick best results - report all runs
- ❌ Ignore outliers without justification - document removal
- ❌ Run benchmarks on busy system - dedicated hardware
- ❌ Skip warmup runs - JIT and caching matter
- ❌ Compare different PostgreSQL versions - same version only

---

## Next Steps

After completion:
- Commit with message: `perf(benchmarks): Validate all performance claims with statistical rigor [PHASE3.1]`
- Publish PERFORMANCE_VALIDATION.md to docs/
- Update README with validated performance numbers
- Proceed to **Phase 3.2: Memory Profiling**

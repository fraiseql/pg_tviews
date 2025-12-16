#!/usr/bin/env python3
"""
Simple benchmark runner for pg_tviews
"""

import subprocess
import statistics
from dataclasses import dataclass
import time
from datetime import datetime


@dataclass
class BenchmarkResult:
    name: str
    implementation: str
    duration_ms: float
    iteration: int


@dataclass
class BenchmarkStats:
    name: str
    implementation: str
    n: int
    mean_ms: float
    median_ms: float
    stddev_ms: float
    min_ms: float
    max_ms: float


class BenchmarkRunner:
    def __init__(self, iterations: int = 10, warmup: int = 2):
        self.iterations = iterations
        self.warmup = warmup

    def run_benchmark(
        self, sql_file: str, implementation: str
    ) -> list[BenchmarkResult]:
        results = []

        # Warmup runs
        print(f"Warming up {sql_file} ({implementation})...")
        for i in range(self.warmup):
            self._execute_benchmark(sql_file)

        # Measured runs
        print(
            f"Running {self.iterations} iterations of {sql_file} ({implementation})..."
        )
        for i in range(self.iterations):
            duration_ms = self._execute_benchmark(sql_file)
            results.append(
                BenchmarkResult(
                    name=sql_file,
                    implementation=implementation,
                    duration_ms=duration_ms,
                    iteration=i,
                )
            )

        return results

    def _execute_benchmark(self, sql_file: str) -> float:
        start = time.perf_counter()
        subprocess.run(["psql", "-f", sql_file, "-q"], check=True, capture_output=True)
        end = time.perf_counter()
        return (end - start) * 1000  # Convert to milliseconds

    def compute_statistics(self, results: list[BenchmarkResult]) -> BenchmarkStats:
        durations = [r.duration_ms for r in results]

        return BenchmarkStats(
            name=results[0].name,
            implementation=results[0].implementation,
            n=len(durations),
            mean_ms=statistics.mean(durations),
            median_ms=statistics.median(durations),
            stddev_ms=statistics.stdev(durations) if len(durations) > 1 else 0,
            min_ms=min(durations),
            max_ms=max(durations),
        )

    def generate_report(self, stats: list[BenchmarkStats], output_file: str):
        with open(output_file, "w") as f:
            f.write("# pg_tviews Performance Validation Report\n\n")
            f.write(f"Generated: {datetime.now().isoformat()}\n\n")

            f.write("## Benchmark Results\n\n")

            for stat in stats:
                f.write(f"### {stat.name} ({stat.implementation})\n\n")
                f.write(f"- **Mean**: {stat.mean_ms:.2f}ms\n")
                f.write(f"- **Median**: {stat.median_ms:.2f}ms\n")
                f.write(f"- **Std Dev**: {stat.stddev_ms:.2f}ms\n")
                f.write(f"- **Min**: {stat.min_ms:.2f}ms\n")
                f.write(f"- **Max**: {stat.max_ms:.2f}ms\n")
                f.write(f"- **Sample Size**: {stat.n}\n\n")


if __name__ == "__main__":
    runner = BenchmarkRunner(iterations=10, warmup=2)

    # Simple test benchmarks
    benchmarks = [
        ("single_row_update_traditional.sql", "traditional"),
        ("single_row_update_pg_tviews.sql", "pg_tviews"),
    ]

    all_stats = []
    for sql_file, impl in benchmarks:
        sql_path = f"test/sql/comprehensive_benchmarks/{sql_file}"
        try:
            results = runner.run_benchmark(sql_path, impl)
            stats = runner.compute_statistics(results)
            all_stats.append(stats)
        except Exception as e:
            print(f"Error running {sql_path}: {e}")

    if all_stats:
        runner.generate_report(all_stats, "PERFORMANCE_VALIDATION.md")
        print("✅ Benchmark report generated: PERFORMANCE_VALIDATION.md")
    else:
        print("❌ No benchmarks completed")

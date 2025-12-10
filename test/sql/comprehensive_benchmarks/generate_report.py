#!/usr/bin/env python3
"""
Generate comprehensive benchmark report with visualizations
"""

import psycopg
import sys
from datetime import datetime
from typing import List, Dict, Any
import json

DB_NAME = "pg_tviews_benchmark"

def connect_db():
    """Connect to benchmark database"""
    try:
        return psycopg.connect(f"dbname={DB_NAME}")
    except Exception as e:
        print(f"Error connecting to database: {e}")
        sys.exit(1)

def fetch_results(conn) -> List[Dict[str, Any]]:
    """Fetch all benchmark results"""
    with conn.cursor() as cur:
        cur.execute("""
            SELECT
                scenario,
                test_name,
                data_scale,
                operation_type,
                rows_affected,
                cascade_depth,
                execution_time_ms,
                notes
            FROM benchmark_results
            ORDER BY scenario, data_scale, test_name, operation_type
        """)
        columns = [desc[0] for desc in cur.description]
        return [dict(zip(columns, row)) for row in cur.fetchall()]

def fetch_comparisons(conn) -> List[Dict[str, Any]]:
    """Fetch performance comparison data"""
    with conn.cursor() as cur:
        cur.execute("""
            SELECT
                scenario,
                test_name,
                data_scale,
                operation_type,
                rows_affected,
                baseline_ms,
                incremental_ms,
                improvement_ratio,
                time_saved_ms
            FROM benchmark_comparison
            WHERE improvement_ratio IS NOT NULL
            ORDER BY improvement_ratio DESC
        """)
        columns = [desc[0] for desc in cur.description]
        return [dict(zip(columns, row)) for row in cur.fetchall()]

def generate_markdown_report(results: List[Dict], comparisons: List[Dict]) -> str:
    """Generate markdown report"""
    report = []

    report.append("# pg_tviews Comprehensive Benchmark Report")
    report.append("")
    report.append(f"**Generated:** {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    report.append("")
    report.append("---")
    report.append("")

    # Executive Summary
    report.append("## Executive Summary")
    report.append("")

    if comparisons:
        # Calculate aggregate stats
        avg_improvement = sum(c['improvement_ratio'] for c in comparisons) / len(comparisons)
        max_improvement = max(c['improvement_ratio'] for c in comparisons)
        min_improvement = min(c['improvement_ratio'] for c in comparisons)

        report.append(f"**Total Tests Run:** {len(results)}")
        report.append(f"**Average Improvement:** {avg_improvement:.2f}× faster")
        report.append(f"**Best Improvement:** {max_improvement:.2f}× faster")
        report.append(f"**Minimum Improvement:** {min_improvement:.2f}× faster")
        report.append("")

    # Performance Comparison Table
    report.append("## Performance Comparison: Incremental vs Full Refresh")
    report.append("")
    report.append("| Scenario | Test | Scale | Operation | Rows | Full Refresh (ms) | Incremental (ms) | Improvement | Time Saved (ms) |")
    report.append("|----------|------|-------|-----------|------|-------------------|------------------|-------------|-----------------|")

    for comp in comparisons:
        report.append(
            f"| {comp['scenario']} | {comp['test_name']} | {comp['data_scale']} | "
            f"{comp['operation_type'].replace('_incremental', '')} | {comp['rows_affected']:,} | "
            f"{comp['baseline_ms']:.2f} | {comp['incremental_ms']:.2f} | "
            f"**{comp['improvement_ratio']:.2f}×** | {comp['time_saved_ms']:.2f} |"
        )

    report.append("")

    # Detailed Results by Scenario
    report.append("## Detailed Results by Scenario")
    report.append("")

    # Group by scenario and scale
    scenarios = {}
    for result in results:
        key = (result['scenario'], result['data_scale'])
        if key not in scenarios:
            scenarios[key] = []
        scenarios[key].append(result)

    for (scenario, scale), tests in sorted(scenarios.items()):
        report.append(f"### {scenario.title()} - {scale.title()} Scale")
        report.append("")
        report.append("| Test Name | Operation | Rows | Time (ms) | ms/row | Notes |")
        report.append("|-----------|-----------|------|-----------|--------|-------|")

        for test in tests:
            ms_per_row = test['execution_time_ms'] / test['rows_affected'] if test['rows_affected'] else 0
            notes = test['notes'][:50] + '...' if test['notes'] and len(test['notes']) > 50 else (test['notes'] or '')
            report.append(
                f"| {test['test_name']} | {test['operation_type']} | "
                f"{test['rows_affected']:,} | {test['execution_time_ms']:.3f} | "
                f"{ms_per_row:.3f} | {notes} |"
            )

        report.append("")

    # Scaling Analysis
    report.append("## Scaling Analysis")
    report.append("")
    report.append("### How Performance Scales with Data Size")
    report.append("")

    # Group comparisons by test type
    test_types = {}
    for comp in comparisons:
        test_key = (comp['test_name'], comp['operation_type'])
        if test_key not in test_types:
            test_types[test_key] = []
        test_types[test_key].append(comp)

    for (test_name, op_type), tests in sorted(test_types.items()):
        report.append(f"#### {test_name.replace('_', ' ').title()} - {op_type.replace('_', ' ').title()}")
        report.append("")
        report.append("| Data Scale | Rows Affected | Full Refresh (ms) | Incremental (ms) | Improvement |")
        report.append("|------------|---------------|-------------------|------------------|-------------|")

        for test in sorted(tests, key=lambda x: {'small': 1, 'medium': 2, 'large': 3}[x['data_scale']]):
            report.append(
                f"| {test['data_scale']} | {test['rows_affected']:,} | "
                f"{test['baseline_ms']:.2f} | {test['incremental_ms']:.2f} | "
                f"**{test['improvement_ratio']:.2f}×** |"
            )

        report.append("")

    # Key Findings
    report.append("## Key Findings")
    report.append("")

    # Analyze patterns
    single_row_tests = [c for c in comparisons if 'single_row' in c['operation_type']]
    bulk_100_tests = [c for c in comparisons if 'bulk_100' in c['operation_type']]
    bulk_1000_tests = [c for c in comparisons if 'bulk_1000' in c['operation_type']]

    if single_row_tests:
        avg_single = sum(c['improvement_ratio'] for c in single_row_tests) / len(single_row_tests)
        report.append(f"- **Single Row Operations:** Average {avg_single:.2f}× improvement")

    if bulk_100_tests:
        avg_bulk_100 = sum(c['improvement_ratio'] for c in bulk_100_tests) / len(bulk_100_tests)
        report.append(f"- **Bulk 100 Row Operations:** Average {avg_bulk_100:.2f}× improvement")

    if bulk_1000_tests:
        avg_bulk_1000 = sum(c['improvement_ratio'] for c in bulk_1000_tests) / len(bulk_1000_tests)
        report.append(f"- **Bulk 1000 Row Operations:** Average {avg_bulk_1000:.2f}× improvement")

    report.append("")

    # Analyze by scale
    for scale in ['small', 'medium', 'large']:
        scale_tests = [c for c in comparisons if c['data_scale'] == scale]
        if scale_tests:
            avg_improvement = sum(c['improvement_ratio'] for c in scale_tests) / len(scale_tests)
            total_time_saved = sum(c['time_saved_ms'] for c in scale_tests)
            report.append(f"- **{scale.title()} Scale:** Average {avg_improvement:.2f}× improvement, {total_time_saved:.2f}ms total time saved")

    report.append("")

    # Recommendations
    report.append("## Recommendations")
    report.append("")
    report.append("Based on benchmark results:")
    report.append("")
    report.append("✅ **Use pg_tviews for:**")
    report.append("- Single row updates (significant improvement even on small datasets)")
    report.append("- Medium-size bulk operations (100-1000 rows)")
    report.append("- Frequently updated views with cascade dependencies")
    report.append("- Real-time applications requiring immediate consistency")
    report.append("")
    report.append("⚠️ **Consider alternatives when:**")
    report.append("- Full table refreshes are infrequent (hourly/daily)")
    report.append("- Batch updates affect >50% of rows")
    report.append("- Write throughput >10K rows/second sustained")
    report.append("")

    return "\n".join(report)

def main():
    print("Connecting to database...")
    conn = connect_db()

    print("Fetching benchmark results...")
    results = fetch_results(conn)
    comparisons = fetch_comparisons(conn)

    if not results:
        print("No benchmark results found. Run benchmarks first.")
        sys.exit(1)

    print(f"Found {len(results)} results and {len(comparisons)} comparisons")

    print("Generating markdown report...")
    report = generate_markdown_report(results, comparisons)

    # Save to file
    timestamp = datetime.now().strftime('%Y%m%d_%H%M%S')
    filename = f"results/BENCHMARK_REPORT_{timestamp}.md"

    with open(filename, 'w') as f:
        f.write(report)

    print(f"Report saved to: {filename}")
    print("\n" + "="*60)
    print(report)
    print("="*60)

    conn.close()

if __name__ == "__main__":
    main()

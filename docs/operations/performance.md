# Performance Documentation

Complete performance optimization resources for pg_tviews.

---

## 📚 Documentation Index

### ⭐ Start Here
- **[Performance Best Practices](performance-best-practices.md)** - Essential patterns and anti-patterns for optimal performance

### 🔧 Tools & Analysis
- **[Performance Analysis](performance-analysis.md)** - Diagnostic tools, monitoring queries, and bottleneck identification
- **[Index Optimization](index-optimization.md)** - Index strategies, maintenance, and automated recommendations
- **[Performance Tuning](performance-tuning.md)** - Advanced tuning for high-throughput applications

### 📊 Benchmarks & Results
- **[Smart Patching Results](../benchmarks/smart-patching-results.md)** - JSONB patching performance benchmarks
- **[Docker Benchmarks](../benchmarks/docker-benchmarks.md)** - Containerized performance testing
- **[Benchmark Overview](../benchmarks/overview.md)** - General benchmarking methodology

---

## Quick Navigation

### By Use Case

**Setting Up a New TVIEW**:
1. Read [Best Practices - Schema Design](performance-best-practices.md#schema-design)
2. Follow [Index Optimization - Index Strategy](index-optimization.md#index-strategy-by-use-case)
3. Use [Performance Analysis - Quick Performance Check](performance-analysis.md#quick-performance-check)

**Debugging Slow Performance**:
1. Run [Performance Analysis - Bottleneck Identification](performance-analysis.md#performance-bottleneck-identification)
2. Check [Index Optimization - Troubleshooting](index-optimization.md#troubleshooting)
3. Review [Best Practices - Anti-Patterns](performance-best-practices.md#anti-patterns-to-avoid)

**Optimizing Existing TVIEWs**:
1. Review [Performance Tuning - Baseline Performance](performance-tuning.md#baseline-performance)
2. Apply [Index Optimization - Index Strategy](index-optimization.md#recommended-manual-indexes)
3. Follow [Best Practices - Performance Checklist](performance-best-practices.md#performance-checklist)

### By Topic

| Topic | Document | Section |
|-------|----------|---------|
| **Index Strategy** | [Index Optimization](index-optimization.md) | All sections |
| **JSONB Performance** | [Best Practices](performance-best-practices.md) | Query Optimization |
| **Cascade Analysis** | [Performance Analysis](performance-analysis.md) | Cascade Dependency Analysis |
| **Memory Tuning** | [Best Practices](performance-best-practices.md) | Memory Management |
| **Bulk Operations** | [Best Practices](performance-best-practices.md) | Bulk Operations |
| **Query Plans** | [Performance Analysis](performance-analysis.md) | Query Plan Analysis |
| **Monitoring** | [Performance Analysis](performance-analysis.md) | Real-Time Monitoring |
| **PostgreSQL Config** | [Best Practices](performance-best-practices.md) | PostgreSQL Configuration |

---

## Performance Targets

### Expected Performance Characteristics

| Operation | Target | Notes |
|-----------|--------|-------|
| Single-row cascade | 0.5-2 ms | With indexes and jsonb_ivm |
| Bulk cascade (1K rows) | 10-50 ms | Depends on cascade depth |
| JSONB query (indexed) | <1 ms | GIN index on data column |
| UUID lookup (indexed) | <1 ms | B-tree index on id column |
| Cache hit ratio | >95% | After warm-up period |

See [Performance Tuning - Baseline Performance](performance-tuning.md#baseline-performance) for detailed benchmarks.

---

## Common Performance Issues

Quick links to solutions:

1. **Slow cascades** → [Best Practices - Index Strategy](performance-best-practices.md#index-strategy)
2. **High memory usage** → [Best Practices - Memory Management](performance-best-practices.md#memory-management)
3. **Poor JSONB query performance** → [Index Optimization - JSONB GIN Indexes](index-optimization.md#2-jsonb-gin-indexes)
4. **Sequential scans on large tables** → [Performance Analysis - Identify Missing Indexes](performance-analysis.md#identify-missing-indexes)
5. **OOM during cascades** → [Analysis - Common Issues](performance-analysis.md#issue-2-high-memory-usage)

---

## Related Documentation

- [Monitoring Guide](monitoring.md) - Production monitoring setup
- [Troubleshooting Guide](troubleshooting.md) - Debug common issues
- [Resource Limits](../reference/limits.md) - Capacity planning and scaling
- [Runbooks](runbooks.md) - Operational procedures

---

**Last Updated**: December 11, 2025 • **Version**: 0.1.0-beta.1

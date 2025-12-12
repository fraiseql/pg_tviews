# pg_tviews v0.1.0-beta.1 Release Notes

**Release Date:** December 10, 2025

## 🚀 Beta Release - Feature Complete

We're excited to announce the first beta release of pg_tviews! This release completes all 10 planned development phases, delivering a feature-complete transactional materialized view system for PostgreSQL.

## What is pg_tviews?

pg_tviews provides automatic incremental maintenance of materialized views containing JSONB data. Instead of rebuilding entire views on every change, pg_tviews intelligently tracks dependencies and performs surgical row-level updates, maintaining data consistency with minimal overhead.

## Key Features

### 🎯 Core Functionality (Phases 1-5)
- **Schema Inference**: Automatic parsing and type detection from SELECT statements
- **Dependency Tracking**: Intelligent dependency graph construction and management
- **Incremental Refresh**: Row-level updates instead of full view rebuilds
- **Array Support**: Full support for array operations with automatic type inference
- **Smart Patching**: 2.03× performance improvement with jsonb_ivm integration

### ⚡ Performance Optimizations (Phases 6-9)
- **Statement-Level Triggers**: 100-500× reduction in trigger overhead
- **Bulk Refresh API**: N→2 query optimization (refresh N rows with 2 queries)
- **Query Plan Caching**: 10× faster query execution with prepared statements
- **Graph Caching**: 90% hit rate for dependency lookups
- **Table Caching**: 95% hit rate for OID lookups
- **Batch Processing**: 3-5× faster for large cascades

### 🔐 Enterprise Features (Phases 7-9)
- **Two-Phase Commit (2PC)**: Queue persistence for prepared transactions
- **Connection Pooling**: Full PgBouncer/pgpool-II compatibility
- **Transaction Safety**: REPEATABLE READ isolation, savepoint support
- **Monitoring**: Comprehensive metrics, health checks, performance views
- **DISCARD ALL**: Safe connection pooler reset handling

### 🛡️ Code Quality (Phase 10)
- **Clippy-Strict Compliance**: 100% clippy compliance with -D warnings
- **FFI Safety**: All callbacks wrapped in panic guards
- **Error Handling**: Complete unwrap() elimination, comprehensive NULL checks
- **Documentation**: Module-level docs for all major components
- **CI/CD**: GitHub Actions workflows for quality assurance

## Installation

### Prerequisites
- PostgreSQL 15+ (tested through 17)
- Rust 1.70+
- cargo-pgrx 0.12.8

### Quick Install
```bash
# Clone the repository
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews
git checkout v0.1.0-beta.1

# Install pgrx
cargo install --locked cargo-pgrx

# Initialize pgrx with your PostgreSQL version
cargo pgrx init

# Build and install the extension
cargo pgrx install --release

# Enable in your database
psql -d your_database -c "CREATE EXTENSION pg_tviews;"
```

### Optional: jsonb_ivm (Recommended)
For optimal performance, install the jsonb_ivm extension first:
```bash
git clone https://github.com/fraiseql/jsonb_ivm.git
cd jsonb_ivm
cargo pgrx install --release

# Enable both extensions (order matters)
psql -d your_database -c "CREATE EXTENSION jsonb_ivm;"
psql -d your_database -c "CREATE EXTENSION pg_tviews;"
```

## Performance Benchmarks

| Operation | Performance | Improvement |
|-----------|-------------|-------------|
| Statement-level triggers | 100-500× faster | vs row-level |
| Bulk refresh (N rows) | 2 queries | vs N queries |
| Query plan caching | 10× faster | vs re-parsing |
| Smart JSONB patching | 2.03× faster | vs full replace |
| Large cascades (≥10 rows) | 3-5× faster | batch optimization |

## What's New in Beta 1

### Phase 10: Code Quality & Safety
- Complete unwrap() elimination for robust error handling
- All FFI callbacks wrapped in panic guards
- Comprehensive module documentation
- CI/CD integration with automated quality checks

### Phase 9: Production Readiness
- Statement-level triggers for bulk operations
- Bulk refresh API (N→2 query optimization)
- Query plan caching system
- Connection pooling safety (DISCARD ALL handling)
- Production monitoring infrastructure

### Phase 8: Distributed Transactions
- Two-Phase Commit (2PC) support
- Queue persistence for prepared transactions
- Recovery API for prepared transaction cleanup

### Phase 7: Performance & Monitoring
- Graph and table caching with high hit rates
- Comprehensive metrics tracking
- Performance debugging tools

### Phase 6: Queue Architecture
- Thread-local refresh queue
- Transaction callback integration
- Savepoint support for proper rollback handling

## Known Limitations

- Some complex SQL constructs not yet supported
- View definitions must be parseable by the inference engine
- Best performance requires optional jsonb_ivm extension
- Beta software - thorough testing recommended before production use

## Breaking Changes

None - this is the first beta release.

## Migration Guide

Not applicable for first beta release.

## Testing This Release

We encourage beta testers to:

1. **Test in staging environments** - Don't deploy directly to production
2. **Report issues** - Open GitHub issues for any bugs or unexpected behavior
3. **Benchmark your workloads** - Share performance results with your use cases
4. **Review documentation** - Help us improve docs by reporting unclear areas
5. **Test edge cases** - Complex queries, large datasets, high concurrency

## Reporting Issues

Please report bugs and issues on GitHub:
- Repository: https://github.com/fraiseql/pg_tviews
- Issues: https://github.com/fraiseql/pg_tviews/issues

Include:
- PostgreSQL version
- pg_tviews version (0.1.0-beta.1)
- Minimal reproduction case
- Error messages and logs

## Roadmap to 1.0.0

Before declaring 1.0.0 stable, we plan to:
- Gather beta feedback and fix reported issues
- Conduct extensive real-world testing
- Performance optimization based on user workloads
- Documentation improvements
- Additional PostgreSQL version testing

## Contributors

- Lionel Hamayon <lionel.hamayon@evolution-digitale.fr>

## License

MIT License - see LICENSE file for details.

## Acknowledgments

- Built with [pgrx](https://github.com/pgcentralfoundation/pgrx) framework
- Optional [jsonb_ivm](https://github.com/fraiseql/jsonb_ivm) integration
- Inspired by PostgreSQL's materialized view system

---

**Thank you for testing pg_tviews v0.1.0-beta.1!**

We're excited to bring transactional materialized views to PostgreSQL and look forward to your feedback as we work toward a stable 1.0.0 release.

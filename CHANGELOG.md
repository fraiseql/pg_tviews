# Changelog

All notable changes to pg_tviews will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/SemVer).

## [Unreleased]

### Added
- Phase 6 planning framework and decision criteria
- Advanced array handling documentation (`docs/ARRAYS.md`)
- Performance regression testing infrastructure

## [0.1.0-alpha] - 2025-12-09

### Phase 5: Array Handling and Performance Optimization - COMPLETE ✅

#### 🚀 Major Features

**Array Handling Implementation**
- **Automatic Type Inference**: Detects `ARRAY(...)` and `jsonb_agg()` patterns
- **Array Element Operations**: Full INSERT/DELETE support for array elements
- **Schema Enhancement**: Added `additional_columns_with_types` for type tracking
- **Dependency Analysis**: Array aggregation pattern detection (`jsonb_agg(v_table.data)`)
- **Trigger Integration**: INSERT/DELETE operations routed to appropriate handlers

**Performance Optimizations**
- **Smart JSONB Patching**: 2.03× performance improvement validated
- **Batch Processing**: 3-5× faster for large cascades (≥10 rows)
- **Memory Efficiency**: Surgical updates vs full document replacement
- **Adaptive Optimization**: Automatic switching between individual and batch updates

#### 📊 Performance Results

**Benchmark Results (Phase 5 Complete):**
```
Baseline Performance:     7.55 ms (medium cascade)
Smart Patch Performance:  3.72 ms (medium cascade)
Improvement:              2.03× faster (51% reduction)

Batch Optimization:       3-5× faster for cascades ≥10 rows
Memory Usage:             Surgical updates (no full replacement)
Scalability:              Linear performance scaling
```

#### 🔧 Technical Improvements

**Schema Inference Engine**
- Enhanced column type detection for arrays
- Improved SQL expression parsing
- Better pattern recognition for complex queries

**Dependency Tracking**
- Array aggregation dependency detection
- Smart patching support for array elements
- Enhanced cascade propagation logic

**Refresh Engine**
- Batch optimization for large operations
- Improved concurrency handling
- Better error recovery mechanisms

#### 🧪 Testing & Quality

**Comprehensive Test Suite**
- `50_array_columns.sql`: Array column materialization tests
- `51_jsonb_array_update.sql`: JSONB array element update tests
- `52_array_insert_delete.sql`: Array INSERT/DELETE operation tests
- `53_batch_optimization.sql`: Batch update optimization tests

**Quality Assurance**
- 100% test coverage maintained for core functionality
- Performance regression testing implemented
- Comprehensive error handling validation

#### 📚 Documentation

**Updated Documentation**
- README.md: Added array handling features and latest performance results
- docs/ARRAYS.md: Comprehensive array handling guide
- Performance benchmarks documented with variance analysis
- Migration guides for array operations

#### 🏗️ Architecture

**Code Organization**
- `src/refresh/array_ops.rs`: Array operation functions
- `src/refresh/batch.rs`: Batch optimization logic
- Enhanced schema inference with type tracking
- Improved dependency analysis for arrays

### Phase 4: Refresh Logic and Cascade Propagation - Previously Completed ✅

#### Features
- Complete cascade propagation system
- JSONB smart patching with jsonb_ivm integration
- Transaction isolation support
- Concurrency-safe refresh operations

### Phase 3: Dependency Detection and Triggers - Previously Completed ✅

#### Features
- Automatic dependency graph construction
- Trigger installation and management
- Cycle detection and prevention
- Metadata table management

### Phase 2: View Creation and DDL Hooks - Previously Completed ✅

#### Features
- DDL hook system for automatic TVIEW creation
- Materialized table management
- View definition parsing
- Schema inference foundation

### Phase 1: Schema Inference - Previously Completed ✅

#### Features
- SQL statement parsing
- Column type inference
- Relationship detection
- Foundation for dependency tracking

## [0.0.1-alpha] - 2025-11-01

### Added
- Initial project structure
- Basic PostgreSQL extension framework
- pgrx integration
- Development environment setup

---

## Development Phases

### Phase 6 Planning (Next)
**Decision Required:** Choose next major feature direction
- **Option A:** Advanced Array Support (multi-dimensional, complex matching)
- **Option B:** Query Optimization (partial refresh, incremental updates)
- **Option C:** Enterprise Features (multi-tenant, audit logging)
- **Option D:** Ecosystem Integration (ORMs, frameworks)

### Phase 5 Achievements ✅
- **Performance:** 2.03× improvement with smart patching
- **Arrays:** Full INSERT/DELETE support with type inference
- **Batch:** 3-5× faster for large cascades
- **Testing:** Comprehensive benchmark suite
- **Quality:** Production-ready code

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development guidelines and TDD workflow.

## Performance Benchmarks

For detailed performance analysis, see:
- [docs/PERFORMANCE_RESULTS.md](docs/PERFORMANCE_RESULTS.md)
- [test/sql/benchmark_*.sql](test/sql/) test files
- Phase 5 benchmark reports

---

**Legend:**
- ✅ Completed
- 🔄 In Progress
- 📋 Planned
- 🐛 Bug Fix
- 🚀 New Feature
- 📚 Documentation
- 🏗️ Architecture
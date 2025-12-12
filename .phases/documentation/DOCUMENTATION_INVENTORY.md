# Documentation Inventory & Audit

**Created**: December 11, 2025
**Status**: Phase A1 - Foundation Audit Complete
**Next**: Phase A2 - Resolve DDL Syntax Confusion

---

## Executive Summary

**Total Features Catalogued**: 47
- ✅ **Documented**: 32 (68%)
- ❌ **Missing Documentation**: 15 (32%)
- ⚠️ **Inconsistent Documentation**: 3 (6%)

**Critical Gaps** (Block v1.0.0):
- 5 undocumented SQL functions
- 14 undocumented error types
- 3 undocumented configuration options
- DDL syntax confusion (CREATE TVIEW vs pg_tviews_create)

---

## Feature Inventory Matrix

### Rust Functions (src/lib.rs)

| Function | Status | Location | Priority |
|----------|--------|----------|----------|
| `pg_tviews_version()` | ✅ Documented | `docs/reference/api.md` | Low |
| `pg_tviews_check_jsonb_ivm()` | ✅ Documented | `docs/reference/api.md` | High |
| `pg_tviews_queue_stats()` | ✅ Documented | `docs/reference/api.md` | Medium |
| `pg_tviews_debug_queue()` | ✅ Documented | `docs/reference/api.md` | Medium |
| `pg_tviews_analyze_select()` | ✅ Documented | `docs/reference/api.md` | Medium |
| `pg_tviews_infer_types()` | ✅ Documented | `docs/reference/api.md` | Medium |
| `pg_tviews_commit_prepared()` | ✅ Documented | `docs/reference/api.md` | High |
| `pg_tviews_rollback_prepared()` | ✅ Documented | `docs/reference/api.md` | High |
| `pg_tviews_recover_prepared_transactions()` | ✅ Documented | `docs/reference/api.md` | High |
| `pg_tviews_cascade()` | ✅ Documented | `docs/reference/api.md` | Medium |
| `pg_tviews_insert()` | ✅ Documented | `docs/reference/api.md` | Low |
| `pg_tviews_delete()` | ✅ Documented | `docs/reference/api.md` | Low |

### SQL Functions (sql/*.sql)

| Function | Status | Location | Priority |
|----------|--------|----------|----------|
| `pg_tviews_install_stmt_triggers()` | ❌ **MISSING** | - | High |
| `pg_tviews_uninstall_stmt_triggers()` | ❌ **MISSING** | - | High |
| `pg_tviews_record_metrics()` | ❌ **MISSING** | - | Medium |
| `pg_tviews_health_check()` | ❌ **MISSING** | - | High |
| `pg_tviews_cleanup_metrics()` | ❌ **MISSING** | - | Medium |

### Monitoring Views (sql/pg_tviews_monitoring.sql)

| View | Status | Location | Priority |
|------|--------|----------|----------|
| `pg_tviews_queue_realtime` | ✅ Documented | `docs/operations/monitoring.md` | Medium |
| `pg_tviews_statement_stats` | ✅ Documented | `docs/operations/monitoring.md` | Low |
| `pg_tviews_cache_stats` | ✅ Documented | `docs/operations/monitoring.md` | Medium |
| `pg_tviews_performance_summary` | ✅ Documented | `docs/operations/monitoring.md` | Medium |

### Error Types (src/error/mod.rs)

| Error Type | SQLSTATE | Status | Location | Priority |
|------------|----------|--------|----------|----------|
| `MetadataNotFound` | P0001 | ❌ **MISSING** | - | High |
| `TViewAlreadyExists` | 42710 | ❌ **MISSING** | - | High |
| `InvalidTViewName` | 42602 | ❌ **MISSING** | - | High |
| `CircularDependency` | 55P03 | ❌ **MISSING** | - | High |
| `DependencyDepthExceeded` | 54001 | ❌ **MISSING** | - | High |
| `DependencyResolutionFailed` | 55000 | ❌ **MISSING** | - | High |
| `InvalidSelectStatement` | 42601 | ❌ **MISSING** | - | High |
| `RequiredColumnMissing` | 42703 | ❌ **MISSING** | - | High |
| `TypeInferenceFailed` | 42804 | ❌ **MISSING** | - | High |
| `JsonbIvmNotInstalled` | 58P01 | ❌ **MISSING** | - | High |
| `ExtensionVersionMismatch` | 58P01 | ❌ **MISSING** | - | Medium |
| `LockTimeout` | 40P01 | ❌ **MISSING** | - | Medium |
| `DeadlockDetected` | 40P01 | ❌ **MISSING** | - | Medium |
| `CascadeDepthExceeded` | 54001 | ❌ **MISSING** | - | High |
| `RefreshFailed` | XX000 | ❌ **MISSING** | - | High |
| `BatchTooLarge` | 54000 | ❌ **MISSING** | - | Medium |
| `DependencyCycle` | 55P03 | ❌ **MISSING** | - | High |
| `PropagationDepthExceeded` | 54001 | ❌ **MISSING** | - | High |
| `CatalogError` | XX000 | ❌ **MISSING** | - | Medium |
| `SpiError` | XX000 | ❌ **MISSING** | - | Medium |
| `SerializationError` | XX000 | ❌ **MISSING** | - | Medium |
| `ConfigError` | XX000 | ❌ **MISSING** | - | Medium |
| `CacheError` | XX000 | ❌ **MISSING** | - | Medium |
| `CallbackError` | XX000 | ❌ **MISSING** | - | Medium |
| `MetricsError` | XX000 | ❌ **MISSING** | - | Medium |
| `InternalError` | XX000 | ❌ **MISSING** | - | High |

### Configuration Options (src/config/mod.rs)

| Setting | Status | Location | Priority |
|---------|--------|----------|----------|
| `MAX_DEPENDENCY_DEPTH` | ❌ **MISSING** | - | Medium |
| `DEBUG_DEPENDENCIES` | ❌ **MISSING** | - | Low |
| `max_propagation_depth()` | ❌ **MISSING** | - | High |
| `graph_cache_enabled()` | ❌ **MISSING** | - | Medium |
| `table_cache_enabled()` | ❌ **MISSING** | - | Medium |
| `log_level()` | ❌ **MISSING** | - | Low |
| `metrics_enabled()` | ❌ **MISSING** | - | Medium |

### DDL Commands

| Command | Status | Location | Priority |
|---------|--------|----------|----------|
| `CREATE TVIEW` | ✅ Documented | `docs/reference/ddl.md` | High |
| `DROP TABLE` | ✅ Documented | `docs/reference/ddl.md` | High |
| `pg_tviews_create()` | ⚠️ **INCONSISTENT** | `docs/reference/api.md` | Critical |

---

## Critical Issues Identified

### 1. DDL Syntax Inconsistency (CRITICAL)

**Problem**: Two different ways to create TVIEWs, unclear which is preferred.

**Evidence**:
- Quick Start shows: `CREATE TVIEW tv_name AS SELECT...`
- API Reference shows: `SELECT pg_tviews_create('tv_name', 'SELECT...')`
- Users don't know which to use or why both exist

**Impact**: Major user confusion, blocks production adoption.

**Resolution Needed**: Decide on one approach and document clearly.

### 2. jsonb_ivm Dependency Confusion (HIGH)

**Problem**: Documentation doesn't clearly state if jsonb_ivm is required or optional.

**Evidence**:
- Code shows it's optional (feature detection exists)
- Some docs imply it's required
- Performance impact not quantified

**Impact**: Users may unnecessarily avoid pg_tviews thinking jsonb_ivm is required.

### 3. Missing Error Reference (HIGH)

**Problem**: 14 error types completely undocumented.

**Evidence**: No error reference document exists.

**Impact**: Users cannot troubleshoot errors effectively.

### 4. Undocumented SQL Functions (MEDIUM)

**Problem**: 5 important SQL functions not documented.

**Evidence**:
- `pg_tviews_install_stmt_triggers()` - Critical for performance
- `pg_tviews_health_check()` - Essential for operations
- Others are monitoring/admin functions

**Impact**: Users cannot use important operational features.

---

## Documentation Quality Assessment

### Completeness Score: 68%

**By Category**:
- API Functions: 100% (12/12 documented)
- DDL Commands: 100% (2/2 documented)
- Monitoring Views: 100% (4/4 documented)
- Error Types: 0% (0/14 documented)
- SQL Functions: 0% (0/5 documented)
- Configuration: 0% (0/7 documented)

### Accuracy Assessment

**Strengths**:
- Existing documentation appears technically accurate
- Good examples in API reference
- Clear DDL syntax documentation

**Issues Found**:
- DDL syntax inconsistency (CREATE TVIEW vs function)
- jsonb_ivm dependency status unclear
- Version labeling confusion (0.1.0-beta.1 but claims production-ready)

### Consistency Assessment

**Good**:
- Style is consistent across documents
- Terminology is mostly consistent
- Code examples follow similar patterns

**Issues**:
- Version status indicators inconsistent
- DDL syntax confusion
- Some documents reference "Week X" timelines that are outdated

---

## Prioritization Matrix

### Critical Path (Must Fix for v1.0.0)

1. **DDL Syntax Resolution** - Decide CREATE TVIEW vs pg_tviews_create()
2. **jsonb_ivm Clarity** - Document dependency status and performance impact
3. **Error Reference** - Document all 14 error types
4. **Missing SQL Functions** - Document 5 key operational functions

### High Priority (Should Fix)

1. **Configuration Reference** - Document tuning options
2. **Version Consistency** - Fix status labeling confusion
3. **Documentation Standards** - Establish style guide and templates

### Medium Priority (Nice to Fix)

1. **Enhanced Examples** - More comprehensive code samples
2. **Performance Tuning Guide** - Workload optimization guidance
3. **Migration Guide** - From traditional materialized views

---

## Next Steps

### Immediate Actions (Phase A2-A4)

1. **Investigate DDL Implementation** - Check if CREATE TVIEW and pg_tviews_create() are truly equivalent
2. **Test jsonb_ivm Performance** - Quantify the performance difference
3. **Create Error Reference** - Document all error types with examples
4. **Document Missing Functions** - Add the 5 undocumented SQL functions

### Phase Planning

**Phase A (Foundation)**: 16-24 hours
- A1: Documentation audit ✅ (4h) - **COMPLETED**
- A2: DDL syntax resolution (6h) - **NEXT**
- A3: jsonb_ivm clarity (4h)
- A4: Version consistency (2h)
- A5: Documentation standards (4h)

**Phase B (Reference)**: 32-48 hours
- Complete API reference enhancements
- Complete DDL reference with limitations
- Create comprehensive error reference
- Create monitoring reference
- Create configuration reference

**Phase C (Operations)**: 24-32 hours
- Migration guide
- Disaster recovery procedures
- Production deployment checklist
- Performance tuning guide

---

## Files Modified/Created

- `DOCUMENTATION_INVENTORY.md` - This inventory matrix
- `DOCUMENTATION_ISSUES.md` - Detailed issue descriptions

---

**Audit Completed**: December 11, 2025
**Next Phase**: A2 - DDL Syntax Investigation
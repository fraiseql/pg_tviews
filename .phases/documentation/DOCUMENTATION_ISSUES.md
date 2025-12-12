# Documentation Issues & Inconsistencies

**Created**: December 11, 2025
**Status**: Identified during Phase A1 audit
**Next**: Phase A2 - DDL Syntax Resolution

---

## Critical Issues (Block v1.0.0)

### Issue #1: DDL Syntax Inconsistency (CRITICAL)

**Description**: Two different syntaxes for creating TVIEWs, unclear which is official.

**Evidence**:
```sql
-- Method 1: DDL Syntax (shown in Quick Start)
CREATE TABLE tv_posts AS SELECT ...;

-- Method 2: Function Call (shown in API Reference)
SELECT pg_tviews_create('tv_posts', 'SELECT ...');
```

**Impact**:
- User confusion about which approach to use
- Documentation contradicts itself
- Blocks production adoption due to uncertainty

**Investigation Needed**:
1. Are both methods equivalent?
2. Does CREATE TVIEW use ProcessUtility hook?
3. Is pg_tviews_create() just a wrapper?
4. Which should be the "blessed" approach?

**Resolution Options**:
- **Option A**: Both valid, document both clearly
- **Option B**: One is deprecated, mark accordingly
- **Option C**: One is internal, hide from users

### Issue #2: jsonb_ivm Dependency Status Unclear (HIGH)

**Description**: Documentation doesn't clearly state if jsonb_ivm is required or optional.

**Evidence**:
- Code has feature detection: `check_jsonb_ivm_available()`
- Performance warning when not installed
- Some docs imply it's required for basic functionality

**Current Documentation**:
- README mentions it provides "performance optimizations"
- Installation guide doesn't clarify dependency level
- No performance comparison quantified

**Impact**:
- Users may avoid pg_tviews thinking jsonb_ivm is required
- Unclear performance expectations
- Installation decisions made without data

**Investigation Needed**:
1. Test performance with/without jsonb_ivm
2. Quantify performance impact
3. Determine if it's truly optional

### Issue #3: Missing Error Reference (HIGH)

**Description**: Complete lack of error documentation.

**Evidence**:
- 14 error types defined in `src/error/mod.rs`
- No error reference document exists
- Users cannot troubleshoot errors effectively

**Missing Documentation**:
- Error codes and SQLSTATEs
- Cause and solution for each error
- Prevention strategies
- Common troubleshooting scenarios

**Impact**:
- Poor user experience when errors occur
- Support burden increases
- Cannot write proper error handling code

### Issue #4: Undocumented SQL Functions (MEDIUM)

**Description**: 5 important SQL functions completely undocumented.

**Missing Functions**:
1. `pg_tviews_install_stmt_triggers()` - Critical for bulk performance
2. `pg_tviews_uninstall_stmt_triggers()` - Revert to row-level triggers
3. `pg_tviews_health_check()` - System health validation
4. `pg_tviews_record_metrics()` - Internal metrics recording
5. `pg_tviews_cleanup_metrics()` - Data retention management

**Impact**:
- Users cannot use important operational features
- Performance tuning impossible without statement triggers
- No way to validate system health

---

## Inconsistency Issues

### Issue #5: Version Status Confusion (MEDIUM)

**Description**: Version labeled as beta but documentation claims production-ready.

**Evidence**:
- Version: `0.1.0-beta.1`
- README claims: "Production-ready transactional materialized views"
- Status tables show outdated "Week X" references

**Impact**:
- User confusion about stability
- Unclear upgrade path to 1.0.0
- Mixed messaging about readiness

### Issue #6: Outdated Status Indicators (LOW)

**Description**: Documentation contains outdated status references.

**Evidence**:
- References to "Week 2", "Week 3" development phases
- Status tables not synchronized
- Timeline references that are no longer relevant

**Impact**:
- Documentation appears stale
- User confusion about current status
- Maintenance burden

---

## Missing Documentation Issues

### Issue #7: Configuration Reference Missing (MEDIUM)

**Description**: No documentation of configuration options.

**Missing**:
- `MAX_DEPENDENCY_DEPTH` constant
- `max_propagation_depth()` setting
- Cache enable/disable flags
- Logging configuration
- Metrics collection settings

**Impact**:
- Cannot tune performance
- Cannot troubleshoot configuration issues
- Advanced features unusable

### Issue #8: Security Model Undocumented (MEDIUM)

**Description**: No documentation of security considerations.

**Missing**:
- Permission requirements for TVIEW creation
- RLS (Row Level Security) behavior
- Security best practices
- SQL injection prevention
- Audit logging capabilities

**Impact**:
- Cannot deploy securely in production
- Unknown security implications
- Compliance concerns

---

## Quality Issues

### Issue #9: Code Examples Not Tested (LOW)

**Description**: Code examples may not be validated.

**Evidence**:
- No CI validation of SQL examples
- Examples created before full implementation
- Potential syntax errors or outdated patterns

**Impact**:
- Broken examples frustrate users
- Copy-paste code doesn't work
- Increased support burden

### Issue #10: Inconsistent Terminology (LOW)

**Description**: Some inconsistent use of terms.

**Evidence**:
- "TVIEW" vs "tview" vs "transactional view"
- "Materialized view" vs "transactional materialized view"
- "Cascade" vs "refresh propagation"

**Impact**:
- User confusion
- Searchability issues
- Professional appearance

---

## Resolution Priority Matrix

| Issue | Priority | Effort | Impact | Timeline |
|-------|----------|--------|--------|----------|
| DDL Syntax | Critical | High | High | Immediate |
| jsonb_ivm Status | High | Medium | High | Week 1 |
| Error Reference | High | High | High | Week 1-2 |
| SQL Functions | Medium | Medium | Medium | Week 2 |
| Version Status | Medium | Low | Medium | Week 1 |
| Configuration | Medium | Medium | Medium | Week 2 |
| Security Model | Medium | High | Medium | Week 3 |
| Status Updates | Low | Low | Low | Ongoing |
| Example Testing | Low | Medium | Medium | Week 4 |
| Terminology | Low | Low | Low | Ongoing |

---

## Investigation Plan

### DDL Syntax Investigation (A2)

**Questions to Answer**:
1. Does `CREATE TVIEW` use a ProcessUtility hook?
2. Is `pg_tviews_create()` just a programmatic wrapper?
3. Are they functionally identical?
4. Which one should be the primary interface?

**Testing Plan**:
```sql
-- Test both approaches
CREATE TABLE tb_test (pk_test INT PRIMARY KEY, data TEXT);

-- Method 1: DDL
CREATE TABLE tv_test1 AS
SELECT pk_test, jsonb_build_object('data', data) as data FROM tb_test;

-- Method 2: Function
SELECT pg_tviews_create('tv_test2',
    'SELECT pk_test, jsonb_build_object(''data'', data) as data FROM tb_test');

-- Compare results
SELECT * FROM tv_test1;
SELECT * FROM tv_test2;
```

### jsonb_ivm Performance Testing (A3)

**Benchmark Plan**:
```sql
-- Create test data
CREATE TABLE tb_perf (pk_perf BIGSERIAL PRIMARY KEY, id UUID DEFAULT gen_random_uuid(), data JSONB);
INSERT INTO tb_perf (data) SELECT jsonb_build_object('field_' || i, 'value_' || i) FROM generate_series(1,10000) i;

-- Test without jsonb_ivm
DROP EXTENSION IF EXISTS jsonb_ivm;
CREATE TABLE tv_perf AS SELECT pk_perf, id, data FROM tb_perf;

-- Measure update performance
EXPLAIN ANALYZE UPDATE tb_perf SET data = jsonb_set(data, '{field_1}', '"new_value"') WHERE pk_perf = 1;

-- Install jsonb_ivm and retest
CREATE EXTENSION jsonb_ivm;
DROP TABLE tv_perf;
CREATE TABLE tv_perf AS SELECT pk_perf, id, data FROM tb_perf;

-- Measure update performance again
EXPLAIN ANALYZE UPDATE tb_perf SET data = jsonb_set(data, '{field_1}', '"new_value"') WHERE pk_perf = 1;
```

---

## Files to Create/Update

### New Files Needed
- `docs/reference/errors.md` - Error reference
- `docs/reference/configuration.md` - Configuration reference
- `docs/operations/security.md` - Security guide
- `docs/style-guide.md` - Documentation standards

### Files to Update
- `docs/reference/api.md` - Add missing SQL functions
- `docs/reference/ddl.md` - Resolve syntax confusion
- `README.md` - Clarify jsonb_ivm status, fix version messaging
- `docs/getting-started/installation.md` - Update dependency guidance

---

**Issues Identified**: December 11, 2025
**Next Action**: Begin DDL syntax investigation
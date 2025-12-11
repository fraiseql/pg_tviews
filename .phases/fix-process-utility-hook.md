# Fix ProcessUtility Hook for TVIEW DDL Creation

**Created**: December 11, 2025
**Priority**: High (Critical for DDL user experience)
**Architect**: Senior World-Level PostgreSQL Extension Architect
**Timeline**: 2-3 weeks
**Risk**: Medium (Hook debugging is complex but contained)

---

## Executive Summary

The ProcessUtility hook in pg_tviews is designed to intercept `CREATE TABLE tv_* AS SELECT` statements and automatically convert them to TVIEW creation, providing DDL-like syntax. However, the hook currently fails to intercept these statements, falling back to normal table creation without TVIEW functionality.

**Current State**:
- ✅ DROP TABLE tv_* interception works perfectly
- ❌ CREATE TABLE tv_* AS SELECT interception fails silently
- ✅ pg_tviews_create() function works reliably
- ⚠️ Hook appears installed but not triggered for CREATE TABLE AS

**Goal**: Fix the ProcessUtility hook to reliably intercept and handle CREATE TABLE tv_* AS SELECT statements, providing seamless DDL syntax for TVIEW creation.

---

## Required Expertise

**Senior World-Level PostgreSQL Extension Architect** with deep expertise in:

### Core Competencies
- **PostgreSQL Internals**: ProcessUtility hook architecture, node tag handling, statement parsing
- **pgrx Framework**: Rust bindings for PostgreSQL extensions, FFI safety, memory management
- **Hook Debugging**: ProcessUtility hook lifecycle, interception patterns, debugging techniques
- **DDL Processing**: CREATE TABLE AS statement structure, IntoClause handling, RangeVar parsing

### Specific Experience Required
- **Extension Development**: 5+ years building PostgreSQL extensions with pgrx
- **Hook Implementation**: Successfully implemented ProcessUtility hooks in production extensions
- **PostgreSQL Version Compatibility**: Deep knowledge of PostgreSQL 17 internals
- **Rust FFI**: Expert-level unsafe Rust code for PostgreSQL FFI boundaries

---

## Problem Analysis

### Current Hook Implementation

**Location**: `src/hooks.rs`
**Hook Installation**: `_PG_init()` → `install_hook()` → `pg_sys::ProcessUtility_hook`
**Interception Logic**:
```rust
if node_tag == pg_sys::NodeTag::T_CreateTableAsStmt {
    let ctas = utility_stmt as *mut pg_sys::CreateTableAsStmt;
    if handle_create_table_as(ctas, query_string) {
        return; // Handled - don't execute normal CREATE TABLE AS
    }
}
```

### Known Working Behavior
- **DROP TABLE tv_***: Successfully intercepted, TVIEW cleanup performed
- **pg_tviews_create()**: Direct function call works perfectly
- **Hook Installation**: Appears successful (logs show installation)

### Broken Behavior
- **CREATE TABLE tv_* AS SELECT**: Not intercepted, falls back to normal table creation
- **No Hook Logs**: "🔧 HOOK CALLED" not logged for CREATE TABLE AS statements
- **No TVIEW Creation**: No metadata, triggers, or backing views created

### Hypothesis Space

1. **Node Tag Mismatch**: T_CreateTableAsStmt not the correct tag for CREATE TABLE AS
2. **Hook Not Called**: ProcessUtility not invoked for CREATE TABLE AS statements
3. **Structure Access Bug**: Incorrect FFI casting or structure traversal
4. **Query Parsing Bug**: SELECT extraction from query string fails
5. **Context Issue**: Hook works in some contexts but not psql execution
6. **Version Compatibility**: pgrx/PG17 compatibility issue

---

## Deliverables Required

### Phase 1: Diagnostic Investigation Plan
**Goal**: Identify root cause of hook failure

**Deliverables**:
- Detailed diagnostic methodology for ProcessUtility hook debugging
- Instrumentation plan for hook execution tracing
- Node tag verification strategy
- FFI structure validation approach
- Context-specific testing matrix

### Phase 2: Root Cause Analysis Report
**Goal**: Pinpoint exact failure point

**Deliverables**:
- Root cause identification with evidence
- Code path analysis from statement execution to hook invocation
- Structure memory layout verification
- Node tag enumeration validation
- Minimal reproduction case

### Phase 3: Fix Implementation Plan
**Goal**: Design and implement the fix

**Deliverables**:
- Detailed fix specification with code changes
- Safety analysis for FFI operations
- Backward compatibility assessment
- Performance impact evaluation
- Testing strategy for fix validation

### Phase 4: Implementation & Testing
**Goal**: Execute fix and validate

**Deliverables**:
- Fixed hook implementation
- Comprehensive test suite
- Performance benchmarking
- Edge case handling
- Documentation updates

### Phase 5: Production Readiness Review
**Goal**: Ensure fix is production-ready

**Deliverables**:
- Security audit of FFI code
- Memory safety verification
- PostgreSQL version compatibility matrix
- Performance regression testing
- Rollback plan if issues discovered

---

## Success Criteria

### Functional Requirements
- [ ] `CREATE TABLE tv_* AS SELECT` statements intercepted reliably
- [ ] TVIEW created with full functionality (metadata, triggers, backing view)
- [ ] Hook works in all execution contexts (psql, applications, scripts)
- [ ] No regression in existing functionality (DROP hook, function approach)
- [ ] Performance impact <5% for non-TVIEW statements

### Quality Requirements
- [ ] Memory safe (no FFI violations, proper cleanup)
- [ ] Thread safe (works in concurrent environments)
- [ ] Exception safe (no panics across FFI boundary)
- [ ] Logging appropriate (debug logs for troubleshooting, no spam)
- [ ] Code documented (complex FFI operations explained)

### Compatibility Requirements
- [ ] PostgreSQL 17+ compatible
- [ ] pgrx framework compatible
- [ ] Backward compatible with existing TVIEWs
- [ ] No breaking changes to public API

---

## Constraints & Assumptions

### Technical Constraints
- **pgrx Version**: Must work with current pgrx 0.12.8
- **PostgreSQL Version**: Target PG17, support PG15+
- **Rust Version**: Compatible with project's Rust 1.70+
- **FFI Safety**: No unsafe operations without thorough review

### Business Constraints
- **Timeline**: 2-3 weeks for complete fix
- **Risk Tolerance**: Medium (hook debugging is complex but contained)
- **Testing**: Must pass existing test suite + new hook tests
- **Documentation**: Must update docs to reflect working DDL syntax

### Assumptions
- **Root Cause**: Hook implementation has a bug (not architectural impossibility)
- **Resources**: Access to PostgreSQL source code and debugging tools
- **Testing Environment**: Full PostgreSQL development environment available
- **Code Access**: Can modify hook implementation and add instrumentation

---

## Risk Assessment

### High Risk Items
- **FFI Memory Corruption**: Incorrect structure access could crash PostgreSQL
- **Hook Not Called**: If ProcessUtility isn't called for CREATE TABLE AS, fix impossible
- **Version Incompatibility**: pgrx/PG17 issues may require framework changes

### Mitigation Strategies
- **Incremental Testing**: Test each change in isolation
- **Instrumentation**: Add extensive logging without affecting performance
- **Fallback Plan**: If hook can't be fixed, document function-only approach
- **Code Review**: Multiple senior architects review FFI changes

---

## Communication Plan

### Weekly Updates
- **Progress Reports**: Current phase status, blockers, discoveries
- **Technical Deep Dives**: When complex issues discovered
- **Risk Updates**: If timeline or success probability changes

### Key Stakeholders
- **Project Lead**: High-level progress and timeline updates
- **QA Team**: Test case development and validation approach
- **DevOps**: Deployment and rollback planning
- **Users**: DDL syntax availability status

---

## Success Metrics

### Quantitative
- **Hook Success Rate**: 100% interception of CREATE TABLE tv_* AS SELECT
- **Performance Impact**: <5% overhead for non-TVIEW statements
- **Test Coverage**: 95%+ coverage of hook code paths
- **Memory Safety**: Zero valgrind errors in hook execution

### Qualitative
- **Code Quality**: Passes senior architect code review
- **Maintainability**: Well-documented complex FFI operations
- **Debuggability**: Clear error messages and logging
- **User Experience**: Seamless DDL syntax for TVIEW creation

---

## Call to Action

**Senior Architect**: Please provide a detailed phased implementation plan to diagnose and fix the ProcessUtility hook for TVIEW DDL creation. Focus on systematic root cause analysis, safe FFI implementation, and comprehensive testing.

**Timeline**: Plan delivery within 2 business days, implementation within 2-3 weeks.

**Priority**: High - This fix enables the promised DDL user experience for TVIEW creation.
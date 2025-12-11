# Phase Doc-4: Error Reference & Debugging

**Phase**: Documentation Phase 4
**Priority**: 🟡 MEDIUM
**Estimated Time**: 3-4 hours
**Status**: NOT STARTED

## Objective

Document all error types, common troubleshooting scenarios, and debugging procedures to help beta testers resolve issues independently.

## Context

The extension has 14 distinct error types implemented in `src/error/mod.rs`, but none are documented. Beta testers need to understand what errors mean and how to resolve them.

## Prerequisites

- Phase Doc-1, Doc-2, Doc-3 completed
- Access to src/error/mod.rs
- Test database for reproducing errors

## Deliverables

1. **`docs/error-reference.md`** - Complete error documentation
2. **`docs/operations/debugging.md`** - Debugging and troubleshooting guide
3. **Updated `README.md`** - Add troubleshooting section

## Implementation Steps

### Step 1: Create Error Reference Structure (20 min)

```markdown
# pg_tviews Error Reference

## Error Types
All errors with codes, causes, and solutions.

## Categories
- Metadata Errors
- Query Validation Errors
- Runtime Errors
- System Errors
```

### Step 2: Document All 14 Error Types (120 min)

Extract from `src/error/mod.rs`:
- MetadataNotFound
- InvalidSelectStatement
- DependencyCycle
- RefreshFailed
- TriggerInstallationFailed
- ViewCreationFailed
- CatalogError
- SpiError
- SerializationError
- ConfigError
- CacheError
- CallbackError
- MetricsError
- InternalError

For each error document:
- Error code (SQLSTATE)
- Common causes
- Example scenarios
- Resolution steps
- When to report as bug

### Step 3: Create Debugging Guide (60 min)

Topics:
- Using pg_tviews_debug_queue()
- Reading log messages
- Common issues and solutions
- Performance debugging
- Dependency troubleshooting

### Step 4: Add Troubleshooting Flowcharts (30 min)

Decision trees for common problems:
- TVIEW not refreshing
- Performance degradation
- Queue buildup
- Trigger issues

### Step 5: Update README (10 min)

Add troubleshooting section with links.

## Acceptance Criteria

- ✅ All 14 error types documented
- ✅ Debugging guide complete
- ✅ Troubleshooting flowcharts included
- ✅ README updated

## Estimated Time: 3-4 hours

## Completion

This completes the critical documentation phases for beta release.

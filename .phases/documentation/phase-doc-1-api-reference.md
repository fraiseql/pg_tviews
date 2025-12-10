# Phase Doc-1: API Reference Documentation

**Phase**: Documentation Phase 1
**Priority**: 🔴 CRITICAL
**Estimated Time**: 4-6 hours
**Status**: NOT STARTED

## Objective

Create comprehensive API reference documentation for all 12 public PostgreSQL functions exposed by the pg_tviews extension. This is the foundation reference that beta testers need to use the extension effectively.

## Context

Currently, only basic extension installation is documented in README.md. Users cannot discover or use the 12 public API functions without reading the source code. This creates a significant barrier to beta testing and adoption.

## Prerequisites

- Access to pg_tviews source code
- PostgreSQL 15+ installation for testing
- pg_tviews extension installed for verification

## Deliverables

1. **`docs/API_REFERENCE.md`** - Complete API documentation
2. **Updated `README.md`** - Add "API Reference" section with link

## Implementation Steps

### Step 1: Create API Reference Document Structure (30 min)

Create `docs/API_REFERENCE.md` with this structure:

```markdown
# pg_tviews API Reference

**Version**: 0.1.0-beta.1
**Last Updated**: [DATE]

## Overview

This document provides complete reference documentation for all public PostgreSQL functions exposed by the pg_tviews extension.

## Function Categories

- [Extension Management](#extension-management)
- [Queue Management](#queue-management)
- [Debugging & Introspection](#debugging--introspection)
- [Two-Phase Commit (2PC)](#two-phase-commit-2pc)
- [Manual Operations](#manual-operations)

## Extension Management

### pg_tviews_version()

[Function details...]

## Queue Management

[Functions...]

## Debugging & Introspection

[Functions...]

## Two-Phase Commit (2PC)

[Functions...]

## Manual Operations

[Functions...]

## See Also

- [Monitoring Guide](MONITORING.md)
- [Operations Guide](OPERATIONS.md)
- [Debugging Guide](DEBUGGING.md)
```

### Step 2: Document Extension Management Functions (45 min)

For each function, provide:
- Function signature
- Description
- Parameters (with types and descriptions)
- Return type
- Usage example
- Notes/warnings

**Functions to document**:

1. **pg_tviews_version()**
```sql
-- Location: src/lib.rs
-- Returns: TEXT
-- Description: Get pg_tviews extension version
-- Example:
SELECT pg_tviews_version();
-- Returns: '0.1.0-beta.1'
```

2. **pg_tviews_check_jsonb_ivm()**
```sql
-- Location: src/lib.rs
-- Returns: BOOLEAN
-- Description: Check if optional jsonb_ivm extension is available
-- Example:
SELECT pg_tviews_check_jsonb_ivm();
-- Returns: true if jsonb_ivm is installed, false otherwise
```

**Documentation Template**:
```markdown
### function_name()

**Signature**:
```sql
function_name(param1 TYPE, param2 TYPE) RETURNS return_type
```

**Description**:
[What the function does, when to use it]

**Parameters**:
- `param1` (TYPE): [Description]
- `param2` (TYPE): [Description]

**Returns**:
- TYPE: [Description of return value]

**Example**:
```sql
[Working example with expected output]
```

**Notes**:
- [Important considerations]
- [Common pitfalls]
- [Performance implications]

**See Also**:
- [Related function](#related-function)
- [Related documentation](../OPERATIONS.md)
```

### Step 3: Document Queue Management Functions (60 min)

**Functions**:

3. **pg_tviews_queue_stats()**
```sql
-- Location: src/lib.rs
-- Returns: TABLE (queue_size INT, total_refreshes BIGINT, ...)
-- Description: Get comprehensive queue statistics
```

4. **pg_tviews_debug_queue()**
```sql
-- Location: src/lib.rs
-- Returns: TABLE (entity TEXT, pk BIGINT, enqueued_at TIMESTAMPTZ)
-- Description: View current contents of refresh queue (debugging)
```

**Research needed**:
- Check exact return type from src/lib.rs
- Test functions to get accurate output
- Document all return columns

### Step 4: Document Debugging & Introspection Functions (45 min)

**Functions**:

5. **pg_tviews_analyze_select(sql TEXT)**
```sql
-- Location: src/lib.rs
-- Returns: JSON or TABLE
-- Description: Analyze a SELECT statement for TVIEW compatibility
```

6. **pg_tviews_infer_types(sql TEXT)**
```sql
-- Location: src/lib.rs
-- Returns: TABLE (column_name TEXT, data_type TEXT, ...)
-- Description: Infer column types from SELECT statement
```

**Research needed**:
- Determine exact return structure
- Test with various SELECT statements
- Document supported vs unsupported SQL features

### Step 5: Document Two-Phase Commit Functions (60 min)

**Functions**:

7. **pg_tviews_commit_prepared(gid TEXT)**
```sql
-- Location: src/lib.rs
-- Returns: VOID
-- Description: Commit a prepared transaction with pending TVIEW refreshes
-- Requires: Prior PREPARE TRANSACTION with GID
```

8. **pg_tviews_rollback_prepared(gid TEXT)**
```sql
-- Location: src/lib.rs
-- Returns: VOID
-- Description: Rollback a prepared transaction, discarding pending refreshes
```

9. **pg_tviews_recover_prepared_transactions()**
```sql
-- Location: src/lib.rs
-- Returns: TABLE (gid TEXT, queue_size INT, status TEXT)
-- Description: List and optionally recover prepared transactions with pending refreshes
```

**Important**: Document 2PC workflow with complete example:
```sql
-- Step 1: Begin transaction with changes
BEGIN;
INSERT INTO posts (title, content) VALUES ('Test', 'Content');

-- Step 2: Prepare transaction (queue is persisted)
PREPARE TRANSACTION 'my-transaction-gid';

-- Step 3a: Commit (in another session or after restart)
SELECT pg_tviews_commit_prepared('my-transaction-gid');

-- OR Step 3b: Rollback
SELECT pg_tviews_rollback_prepared('my-transaction-gid');
```

### Step 6: Document Manual Operations Functions (45 min)

**Functions**:

10. **pg_tviews_cascade(entity TEXT, pk BIGINT)**
```sql
-- Location: src/lib.rs
-- Returns: VOID
-- Description: Manually trigger cascade refresh for an entity+pk
-- Use case: Force refresh after manual data fixes
```

11. **pg_tviews_insert(entity TEXT, pk BIGINT)**
```sql
-- Location: src/lib.rs
-- Returns: VOID
-- Description: Manually trigger insert handling for an entity+pk
```

12. **pg_tviews_delete(entity TEXT, pk BIGINT)**
```sql
-- Location: src/lib.rs
-- Returns: VOID
-- Description: Manually trigger delete handling for an entity+pk
```

**Warning**: Document when these should be used (usually not needed).

### Step 7: Add Usage Examples Section (30 min)

Create "Common Usage Patterns" section with real-world examples:

```markdown
## Common Usage Patterns

### Check Extension Status
```sql
-- Verify extension is installed
SELECT pg_tviews_version();

-- Check for optional performance extension
SELECT pg_tviews_check_jsonb_ivm();
```

### Monitor Queue Activity
```sql
-- Get current queue statistics
SELECT * FROM pg_tviews_queue_stats();

-- View queued refresh operations
SELECT * FROM pg_tviews_debug_queue();
```

### Debug View Definitions
```sql
-- Analyze SELECT for TVIEW compatibility
SELECT pg_tviews_analyze_select('
    SELECT p.id, p.title, u.name as author
    FROM posts p JOIN users u ON p.user_id = u.id
');

-- Check inferred column types
SELECT * FROM pg_tviews_infer_types('SELECT ...') AS t;
```

### Two-Phase Commit Workflow
[Complete 2PC example from Step 5]

### Manual Refresh Operations
```sql
-- Force refresh a specific entity
SELECT pg_tviews_cascade('post', 123);

-- Manually process an insert
SELECT pg_tviews_insert('user', 456);
```
```

### Step 8: Add Notes and Warnings Section (20 min)

```markdown
## Important Notes

### Performance Considerations
- `pg_tviews_debug_queue()` reads thread-local state, no performance impact
- `pg_tviews_queue_stats()` is fast, safe for frequent monitoring
- Manual operations (`pg_tviews_cascade`, etc.) bypass transaction queue
- 2PC functions require careful coordination in distributed systems

### Common Pitfalls
- Don't use manual operations in triggers (causes recursion)
- 2PC GIDs must be unique per prepared transaction
- `pg_tviews_analyze_select()` doesn't validate table existence

### Thread Safety
- Queue functions operate on thread-local state
- Safe for concurrent use across connections
- Each connection has isolated queue state

## Troubleshooting

### Function Not Found
```sql
ERROR:  function pg_tviews_version() does not exist
```
**Solution**: Extension not installed. Run `CREATE EXTENSION pg_tviews;`

### Permission Denied
```sql
ERROR:  permission denied for function pg_tviews_commit_prepared
```
**Solution**: 2PC functions require superuser or specific GRANT permissions.

[More troubleshooting scenarios...]
```

### Step 9: Update README.md (15 min)

Add to README.md after "Basic Usage" section:

```markdown
## API Reference

For complete documentation of all public functions, see [API Reference](docs/API_REFERENCE.md).

**Quick Links**:
- [Extension Management](docs/API_REFERENCE.md#extension-management) - Version info, feature detection
- [Queue Management](docs/API_REFERENCE.md#queue-management) - Monitor refresh queues
- [Debugging](docs/API_REFERENCE.md#debugging--introspection) - Analyze queries, debug issues
- [Two-Phase Commit](docs/API_REFERENCE.md#two-phase-commit-2pc) - Distributed transaction support
- [Manual Operations](docs/API_REFERENCE.md#manual-operations) - Force refresh operations

**Key Functions**:
```sql
SELECT pg_tviews_version();              -- Check version
SELECT pg_tviews_queue_stats();          -- Monitor performance
SELECT pg_tviews_debug_queue();          -- Debug queue contents
SELECT pg_tviews_commit_prepared(gid);   -- Commit prepared transaction
```
```

## Verification Steps

### 1. Install Extension and Test Functions
```bash
# Install extension
psql -d test_db -c "CREATE EXTENSION pg_tviews;"

# Test each documented function
psql -d test_db << 'EOF'
-- Test extension management
SELECT pg_tviews_version();
SELECT pg_tviews_check_jsonb_ivm();

-- Test queue functions
SELECT * FROM pg_tviews_queue_stats();
SELECT * FROM pg_tviews_debug_queue();

-- Test analyze functions
SELECT pg_tviews_analyze_select('SELECT 1');

-- Test 2PC functions (requires setup)
BEGIN;
PREPARE TRANSACTION 'test-gid';
SELECT pg_tviews_commit_prepared('test-gid');

-- Test manual operations
-- (requires existing TVIEW)
EOF
```

### 2. Validate Documentation Accuracy
- [ ] Each function signature matches actual implementation
- [ ] All parameters documented with correct types
- [ ] Return types match actual function output
- [ ] Examples produce expected output
- [ ] Cross-references are valid links

### 3. Completeness Check
- [ ] All 12 public functions documented
- [ ] Each function has complete documentation:
  - [ ] Signature
  - [ ] Description
  - [ ] Parameters
  - [ ] Return type
  - [ ] Example
  - [ ] Notes/warnings
- [ ] Common usage patterns included
- [ ] Troubleshooting section present
- [ ] README.md updated with link

### 4. Readability Review
- [ ] Clear, concise language
- [ ] Consistent terminology
- [ ] Proper Markdown formatting
- [ ] Code blocks properly highlighted
- [ ] No typos or grammatical errors

## Acceptance Criteria

Phase Doc-1 is complete when:

- ✅ `docs/API_REFERENCE.md` exists with complete documentation
- ✅ All 12 public functions documented:
  1. pg_tviews_version()
  2. pg_tviews_check_jsonb_ivm()
  3. pg_tviews_queue_stats()
  4. pg_tviews_debug_queue()
  5. pg_tviews_analyze_select()
  6. pg_tviews_infer_types()
  7. pg_tviews_commit_prepared()
  8. pg_tviews_rollback_prepared()
  9. pg_tviews_recover_prepared_transactions()
  10. pg_tviews_cascade()
  11. pg_tviews_insert()
  12. pg_tviews_delete()
- ✅ Each function has: signature, description, parameters, return type, example, notes
- ✅ Common usage patterns section included
- ✅ Troubleshooting section included
- ✅ README.md updated with API reference link
- ✅ All examples tested and verified to work
- ✅ All links and cross-references valid
- ✅ Documentation reviewed for accuracy and clarity

## Success Metrics

- Beta testers can discover all API functions without reading source
- Beta testers can use all functions correctly based on documentation alone
- No "how do I..." questions that should be answered by API reference
- Zero inaccuracies in function signatures or behavior descriptions

## Notes for Implementation

### Research Required

For each function, verify from source code:
1. **Location**: Which file (src/lib.rs, src/ddl/*.rs, etc.)
2. **Exact signature**: Parameter names and types
3. **Return type**: Exact structure, especially for TABLE returns
4. **Implementation details**: What it actually does

### Testing Strategy

Create a test script `test/api_reference_validation.sql`:
```sql
-- Test all documented functions
\echo 'Testing pg_tviews_version...'
SELECT pg_tviews_version();

\echo 'Testing pg_tviews_check_jsonb_ivm...'
SELECT pg_tviews_check_jsonb_ivm();

-- [Continue for all 12 functions]
```

Run after documentation to verify accuracy:
```bash
psql -d test_db -f test/api_reference_validation.sql > api_test_results.txt
```

### Documentation Style Guide

- **Tone**: Professional, clear, concise
- **Audience**: Database administrators and developers
- **Format**: GitHub-flavored Markdown
- **Code blocks**: Use ```sql for SQL examples
- **Emphasis**: **bold** for critical warnings, *italic* for notes
- **Links**: Use relative paths for docs/ files

## Dependencies

- Source code access: `src/lib.rs`, `src/ddl/*.rs`
- Test database for verification
- PostgreSQL client for testing examples

## Estimated Breakdown

- Step 1 (Structure): 30 min
- Step 2 (Extension mgmt): 45 min
- Step 3 (Queue mgmt): 60 min
- Step 4 (Debugging): 45 min
- Step 5 (2PC): 60 min
- Step 6 (Manual ops): 45 min
- Step 7 (Examples): 30 min
- Step 8 (Notes): 20 min
- Step 9 (README update): 15 min
- Verification & testing: 60 min

**Total**: 4-6 hours (depending on testing iterations)

## Next Phase

After completing Phase Doc-1, proceed to:
→ **Phase Doc-2**: SQL Functions & Monitoring Documentation

The API reference provides the foundation that other docs build upon.

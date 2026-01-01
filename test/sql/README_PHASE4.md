# Phase 4 Test Suite

This directory contains SQL integration tests for Phase 4 (Refresh & Cascade Logic).

## Test Organization

### Phase 4 Tests (40-49)

| Test | File | Purpose |
|------|------|---------|
| 40 | `40_refresh_trigger_dynamic_pk.sql` | Dynamic PK extraction in trigger handler |
| 41 | `41_refresh_single_row.sql` | Single row refresh (no cascade) |
| 42 | `42_cascade_fk_lineage.sql` | FK lineage cascade (parent refresh) |
| 43 | `43_cascade_depth_limit.sql` | Cascade depth limiting (max 10) |
| 44 | `44_trigger_cascade_integration.sql` | Full end-to-end integration |

## Running Tests

### Individual Test
```bash
psql -U postgres -f test/sql/40_refresh_trigger_dynamic_pk.sql
```

### All Phase 4 Tests
```bash
for i in {40..44}; do
    echo "Running test $i..."
    psql -U postgres -f test/sql/${i}_*.sql
done
```

### Using pgrx Test Framework
```bash
cargo pgrx test
```

## Prerequisites

1. **PostgreSQL 15+** installed
2. **jsonb_delta extension** installed and available
3. **pg_tviews extension** compiled

### Install jsonb_delta

```bash
# Clone and install jsonb_delta
git clone https://github.com/fraiseql/jsonb_delta.git
cd jsonb_delta
cargo pgrx install --release
```

## Test Expectations

### Test 40: Dynamic PK Extraction
- Creates table with non-standard PK name (pk_post, pk_user)
- Trigger should extract PK dynamically, not hardcode
- Should work with any pk_* column name

### Test 41: Single Row Refresh
- Base table UPDATE triggers refresh
- tv_* table updated with new data
- updated_at timestamp changes
- No cascade to other tables

### Test 42: FK Lineage Cascade
- Two-level hierarchy: tb_user → tb_post
- Update user → cascades to all posts with fk_user
- Nested JSONB objects updated (author.name)

### Test 43: Cascade Depth Limit
- Create deep dependency chain (10+ levels)
- Should fail at depth 10 with CascadeDepthExceeded error
- Circuit breaker prevents infinite loops

### Test 44: Full Integration
- Three-level hierarchy: company → user → post
- All operations: INSERT, UPDATE, DELETE
- Cascades propagate correctly
- Performance benchmarked

## Debugging

### Enable Verbose Logging
```sql
SET client_min_messages = DEBUG1;
SET pg_tviews.debug_refresh = true;
```

### Check Trigger Installation
```sql
SELECT tgname, tgrelid::regclass
FROM pg_trigger
WHERE tgname LIKE 'trg_tview_%';
```

### Check Metadata
```sql
SELECT entity, array_length(dependencies, 1) as dep_count
FROM pg_tview_meta;
```

## Common Issues

### jsonb_delta Not Found
```
ERROR: extension "jsonb_delta" is not available
```
**Solution:** Install jsonb_delta extension first

### Isolation Level Warning
```
WARNING: pg_tviews requires REPEATABLE READ isolation
```
**Solution:** Use transactions with proper isolation:
```sql
BEGIN;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;
-- ... test code ...
ROLLBACK;
```

### Cascade Depth Exceeded
```
ERROR: Cascade depth limit exceeded (max 10)
```
**Solution:** This is expected for test 43. For real scenarios, increase limit:
```sql
SET pg_tviews.max_cascade_depth = 20;
```

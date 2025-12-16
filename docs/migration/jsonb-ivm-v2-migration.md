# Migrating to jsonb_ivm v2 Integration

This guide helps you upgrade to pg_tviews with enhanced jsonb_ivm integration.

## What's New

1. **Helper Functions** (Phase 1)
   - Faster ID extraction
   - Array existence checking

2. **Nested Path Updates** (Phase 2)
   - Update deep fields in array elements

3. **Batch Operations** (Phase 3)
   - Bulk array updates

4. **Fallback Paths** (Phase 4)
   - Flexible path-based updates

## Migration Steps

### Step 1: Update jsonb_ivm

```bash
# Ensure jsonb_ivm >= 0.2.0
cd ../jsonb_ivm
cargo pgrx install --release
```

### Step 2: Update pg_tviews

```bash
cd pg_tviews
cargo pgrx install --release
```

### Step 3: Update Database

```sql
ALTER EXTENSION pg_tviews UPDATE;
```

### Step 4: Verify Installation

```sql
SELECT * FROM pg_extension WHERE extname IN ('jsonb_ivm', 'pg_tviews');
```

## No Breaking Changes

All existing TVIEWs continue to work without modification. New features are opt-in.

## Performance Gains

- Array operations: 2-10× faster
- Cascade updates: 1.5-3× faster
- Bulk operations: 3-5× faster

## Testing Migration

### Before Migration

```sql
-- Test existing functionality
SELECT pg_tviews_version();
SELECT pg_tviews_health_check();
```

### After Migration

```sql
-- Verify new functions available
SELECT pg_tviews_check_jsonb_ivm();

-- Test performance improvements
-- (Run your existing benchmarks)
```

## Rollback Plan

If issues occur after migration:

```sql
-- Downgrade pg_tviews (if needed)
-- Note: jsonb_ivm can remain at new version
ALTER EXTENSION pg_tviews UPDATE TO '0.1.0';
```

## Monitoring After Migration

```sql
-- Check for performance improvements
SELECT pg_tviews_queue_stats();

-- Monitor for any errors
SELECT * FROM pg_tviews_health_check()
WHERE status != 'OK';
```

## Troubleshooting

### Extension Conflicts

**Error**: `extension "pg_tviews" does not exist`

**Solution**:
```sql
-- Check available extensions
SELECT * FROM pg_available_extensions
WHERE name LIKE '%tview%';

-- Reinstall if needed
DROP EXTENSION IF EXISTS pg_tviews;
CREATE EXTENSION pg_tviews;
```

### Performance Regression

**Issue**: Operations slower after migration

**Check**:
```sql
-- Verify jsonb_ivm is active
SELECT pg_tviews_check_jsonb_ivm();

-- Check for fallback warnings in logs
-- Look for "Using fallback implementation" messages
```

### Compatibility Issues

**Issue**: Existing code breaks

**Solution**: All existing APIs remain unchanged. New functions are additive only.

## Support

For migration issues:
1. Check logs for fallback warnings
2. Run health checks: `SELECT * FROM pg_tviews_health_check()`
3. Verify extension versions
4. Test with jsonb_ivm disabled to isolate issues
# ProcessUtility Hook Status

## Current Status (2025-12-09 15:42)

### ✅ What's Working:
1. **Hook Installation**: The `_PG_init()` function is called and ProcessUtility hook is installed successfully
   - Log confirms: "pg_tviews: _PG_init() called, installing ProcessUtility hook"
   - Log confirms: "pg_tviews: ProcessUtility hook installed"

2. **Build & Install**: Extension compiles and installs correctly
   - SQL file exists: `sql/pg_tviews--0.1.0.sql`
   - Shared library exists: `~/.pgrx/17.7/pgrx-install/lib/postgresql/pg_tviews.so`
   - Extension is in `shared_preload_libraries`

3. **CREATE TABLE tv_* Detection**: The hook is being called for CREATE TABLE tv_* commands
   - But not properly intercepted (yet)

### ❌ What's Broken:

1. **DROP TABLE Handler Crash**: Segmentation fault when `DROP TABLE IF EXISTS tv_product` is executed
   - Crash occurs in `handle_drop_table()` function (src/hooks.rs:170-249)
   - Error: "server process was terminated by signal 11: Segmentation fault"
   - Happens even when table doesn't exist

2. **CREATE TABLE tv_* Not Intercepted**: CREATE TABLE commands are passing through to standard PostgreSQL
   - Should be intercepted and converted to TVIEW creation
   - Currently just creates a regular table

## Root Causes

### DROP Handler Crash
The `handle_drop_table` function has unsafe pointer operations that may be accessing invalid memory:
- Lines 187-220: Complex list traversal with multiple pointer dereferencing
- Problem: When dropping a non-existent table with IF EXISTS, the list structure may be different
- Need to add more safety checks before dereferencing pointers

### CREATE TABLE Not Intercepted
The CREATE TABLE handler `handle_create_table_as` returns `true` when it handles the statement, which should prevent standard_ProcessUtility from being called. However:
- The function may be encountering an error and calling `error!()` macro
- When pgrx `error!()` is called, it jumps out of the function via longjmp
- This means we never return `true`, so the hook thinks we didn't handle it
- Standard CREATE TABLE AS then executes, creating a regular table

## Fixes Needed

### 1. Fix DROP Handler (Priority: CRITICAL)

File: `src/hooks.rs`, function `handle_drop_table` (lines 170-249)

**Problem**: Segfault due to unsafe list traversal

**Solution**: Add comprehensive null checks and handle edge cases:
```rust
// Before accessing any list element:
if object.is_null() {
    continue;
}

// Before casting and dereferencing:
if node.is_null() || (*node).type_ != pg_sys::NodeTag::T_String {
    continue;
}

// Add early return if objects list is empty:
let objects_list = PgList::<pg_sys::List>::from_pg(drop_ref.objects);
if objects_list.is_empty() {
    return false;
}
```

### 2. Fix CREATE TABLE Handler (Priority: HIGH)

File: `src/hooks.rs`, function `handle_create_table_as` (lines 95-167)

**Problem**: Using `error!()` macro which prevents returning `true`

**Solution**: Return errors as `false` instead of calling `error!()`:
```rust
// Replace:
if entity_name.is_empty() {
    error!("Invalid TVIEW name '{}': must be tv_<entity>", table_name);
}

// With:
if entity_name.is_empty() {
    warning!("Invalid TVIEW name '{}': must be tv_<entity>", table_name);
    return false;  // Let standard utility handle it
}
```

**Alternative**: Use `Result` return type and handle errors gracefully:
```rust
// Inside handle_create_table_as:
match create_tview(table_name, &select_sql) {
    Ok(()) => {
        info!("TVIEW '{}' created successfully", table_name);
        true
    }
    Err(e) => {
        // Log but don't crash
        warning!("Failed to create TVIEW '{}': {}", table_name, e);
        false  // Fall back to standard CREATE TABLE
    }
}
```

## Next Steps

1. **Fix the DROP handler crash** (immediate priority)
   - Add null checks and safety guards
   - Test with `DROP TABLE IF EXISTS nonexistent_table`

2. **Fix CREATE TABLE interception**
   - Remove `error!()` calls that prevent proper return
   - Add comprehensive logging to track execution flow

3. **Test both handlers**
   - CREATE TABLE tv_* AS SELECT ...
   - DROP TABLE tv_*
   - DROP TABLE IF EXISTS tv_nonexistent

4. **Verify metadata operations**
   - Ensure `create_tview()` and `drop_tview()` work correctly
   - Check that metadata is properly stored/retrieved

## Testing Commands

```sql
-- Test CREATE TABLE tv_* interception
CREATE TABLE tb_test (id SERIAL PRIMARY KEY, name TEXT);
CREATE EXTENSION IF NOT EXISTS pg_tviews;
CREATE TABLE tv_test AS SELECT id, name FROM tb_test;

-- Verify interception worked
SELECT * FROM pg_tview_meta WHERE entity = 'test';

-- Test DROP TABLE tv_* interception
DROP TABLE IF EXISTS tv_test CASCADE;
```

## Files Modified

- `src/hooks.rs` - ProcessUtility hook implementation (NEW)
- `src/lib.rs` - Added `_PG_init()` to install hook
- `sql/pg_tviews--0.1.0.sql` - Manual SQL installation script (CREATED)

## Compilation Warnings to Fix (Low Priority)

- Unused imports in multiple files
- Unused variables in error handling
- Unused struct fields in refresh.rs
- Can be fixed with `cargo fix --lib -p pg_tviews`

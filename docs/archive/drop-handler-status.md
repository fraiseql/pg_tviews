# DROP TABLE Handler Investigation

## Current Status (2025-12-09 16:00)

### CREATE TABLE Hook: ✅ WORKING
- Hook successfully intercepts `CREATE TABLE tv_* AS SELECT ...`
- Transforms raw SELECT to TVIEW format
- Creates all objects (view, table, indexes, metadata, triggers)
- No segfaults, clean execution

### DROP TABLE Hook: ❌ NOT BEING CALLED

The DROP handler code is present and enabled in `src/hooks.rs` (lines 73-82), but **the hook is never being invoked**.

#### Evidence:
1. No debug logging appears when `DROP TABLE tv_product CASCADE` is executed
2. Expected messages missing:
   - "Hook detected DROP statement"
   - "handle_drop_table called"
   - "Intercepted DROP TABLE ..."

3. Standard PostgreSQL DROP TABLE executes instead
   - Metadata remains in pg_tview_meta
   - View v_product remains
   - Triggers remain on base table

#### Test Results:
```sql
-- After running: DROP TABLE tv_product CASCADE
SELECT COUNT(*) FROM pg_tview_meta WHERE entity = 'product';
-- Returns: 1 (should be 0)

SELECT COUNT(*) FROM pg_class WHERE relname = 'v_product';
-- Returns: 1 (should be 0)

SELECT COUNT(*) FROM pg_trigger WHERE tgname LIKE '%tview_product%';
-- Returns: 1 (should be 0)
```

Only tv_product table itself is dropped, all associated objects remain.

## Why Isn't the Hook Being Called?

### Hypothesis 1: Hook Order
The ProcessUtility hook might not be called for DROP TABLE when CASCADE is involved, or there's a different hook priority system.

### Hypothesis 2: Transaction Context
When CREATE TABLE succeeds by returning from the hook without calling standard_ProcessUtility, PostgreSQL might not be using hooks for subsequent DDL in the same connection.

### Hypothesis 3: Missing Hook Registration
The hook might need additional registration or configuration for DROP statements.

## Code Analysis

### Hook Installation (src/lib.rs:30-40)
```rust
#[pg_guard]
extern "C" fn _PG_init() {
    pgrx::log!("pg_tviews: _PG_init() called, installing ProcessUtility hook");
    unsafe {
        hooks::install_hook();
    }
    pgrx::log!("pg_tviews: ProcessUtility hook installed");
}
```
✅ This is being called successfully (confirmed by logs)

### Hook Detection Code (src/hooks.rs:73-82)
```rust
// Check for DROP TABLE
if node_tag == pg_sys::NodeTag::T_DropStmt {
    info!("Hook detected DROP statement");  // <-- THIS NEVER APPEARS
    let drop_stmt = utility_stmt as *mut pg_sys::DropStmt;
    if handle_drop_table(drop_stmt, query_string) {
        info!("DROP statement was handled by hook, not calling standard utility");
        return;
    }
    info!("DROP statement not handled by hook, passing through");
}
```
❌ None of these log messages appear

### Handler Implementation (src/hooks.rs:178-267)
The `handle_drop_table()` function is well-implemented with:
- Safe string parsing approach (avoids segfaults)
- Proper query string extraction
- Table name detection
- Calls to drop_tview() function

But it's **never being invoked**.

## Next Steps to Debug

1. **Add logging at hook entry point**
   - Add `info!()` at the very beginning of `tview_process_utility_hook`
   - Check if hook is being called at all for DROP statements

2. **Check node_tag values**
   - Log the actual `node_tag` value for every hook invocation
   - Verify that `T_DropStmt` is the correct enum value

3. **Test without CASCADE**
   - Try `DROP TABLE tv_product;` without CASCADE
   - See if CASCADE changes the code path

4. **Investigate pgrx examples**
   - Look for pgrx extensions that handle DROP statements
   - Check if there's a different pattern needed

5. **PostgreSQL internals research**
   - Understand when ProcessUtility hook is vs isn't called
   - Check if there are restrictions on DDL hooks with dependencies

## Workaround for Now

Since CREATE TABLE hook works perfectly, users can work around the DROP limitation:

```sql
-- Instead of: DROP TABLE tv_product CASCADE;

-- Use the SQL function:
SELECT drop_tview('tv_product', false);
```

The `drop_tview()` SQL function is already exported and works correctly. It's just not being automatically called by the hook.

## Files Modified in This Investigation

- src/hooks.rs:73-82 - Re-enabled DROP handler (was commented out)
- src/hooks.rs:178-267 - Rewrote handle_drop_table to use string parsing (avoids segfaults)
- test_hook_complete.sql - Comprehensive test showing DROP doesn't work
- test_drop_simple.sql - Minimal test case for debugging

## Summary

✅ **What Works:**
- CREATE TABLE tv_* AS SELECT ... (perfect!)
- ProcessUtility hook installation
- No segfaults with new string-parsing approach

❌ **What Doesn't Work:**
- DROP TABLE tv_* CASCADE not intercepted
- Hook literally not being called for DROP statements
- This is a **hook invocation** issue, not a handler implementation issue

The handler code is fine, but it's never getting executed. Need to understand why PostgreSQL/pgrx isn't calling our hook for DROP TABLE.

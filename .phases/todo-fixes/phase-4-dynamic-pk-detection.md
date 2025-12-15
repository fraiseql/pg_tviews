# Phase 4: Implement Dynamic Primary Key Column Detection

## Objective

Replace the hardcoded `"pk_post"` column name in `extract_pk()` with dynamic detection based on the table's actual columns or metadata from `pg_tview_meta`.

## Context

Currently, `src/utils.rs:40-44` has:

```rust
// TODO: detect column name dynamically. For now, assume "pk_*" is "pk_post".
// You might want to store the pk column name in pg_tview_meta.
let pk: i64 = tuple
    .get_by_name("pk_post")? // <-- placeholder: replace per entity
    .expect("pk_post must not be null");
```

This only works for the `post` entity. For proper multi-entity support, we need to:
1. Detect the entity name from the table being modified
2. Find the corresponding `pk_<entity>` column dynamically

## Design Options

### Option A: Derive from table name (Recommended)
- Table `tb_user` → look for column `pk_user`
- Table `tb_post` → look for column `pk_post`
- Simple, follows naming convention, no metadata lookup needed

### Option B: Store PK column in pg_tview_meta
- Add `pk_column TEXT` to metadata
- More flexible but requires schema migration
- Deferred to future version

### Option C: Scan tuple for `pk_*` columns
- Find first column starting with `pk_`
- Works without metadata but less explicit
- Could be ambiguous if multiple `pk_*` columns exist

**We'll implement Option A** as it's simplest and follows existing conventions.

## Files to Modify

| File | Changes |
|------|---------|
| `src/utils.rs` | Update `extract_pk()` to accept entity name parameter |
| `src/lib.rs` | Update trigger functions to pass entity name |

## Implementation Steps

### Step 1: Update extract_pk signature

```rust
/// Extracts primary key from NEW or OLD tuple using naming convention
///
/// Looks for column `pk_<entity>` in the tuple.
///
/// # Arguments
///
/// * `trigger` - The trigger context
/// * `entity` - Entity name (e.g., "user", "post")
///
/// # Returns
///
/// The primary key value as i64, or error if column not found/null.
///
/// # Example
///
/// For entity "user", looks for column "pk_user".
pub fn extract_pk(trigger: &PgTrigger, entity: &str) -> spi::Result<i64> {
    let tuple = trigger
        .new()
        .or(trigger.old())
        .expect("Row must exist for AFTER trigger");

    // Build column name from entity: "user" -> "pk_user"
    let pk_column = format!("pk_{}", entity);

    let pk: i64 = tuple
        .get_by_name(&pk_column)?
        .ok_or_else(|| {
            spi::Error::from(crate::TViewError::SpiError {
                query: format!("extract pk from column {}", pk_column),
                error: format!("Column '{}' is NULL or missing", pk_column),
            })
        })?;

    Ok(pk)
}
```

### Step 2: Add helper to derive entity from table name

```rust
/// Derive entity name from table name using naming convention
///
/// Follows the pattern: `tb_<entity>` → `<entity>`
///
/// # Arguments
///
/// * `table_name` - Full table name (e.g., "tb_user")
///
/// # Returns
///
/// Entity name if table follows convention, None otherwise.
///
/// # Example
///
/// ```
/// derive_entity_from_table("tb_user") // => Some("user")
/// derive_entity_from_table("users")   // => None
/// ```
pub fn derive_entity_from_table(table_name: &str) -> Option<&str> {
    table_name.strip_prefix("tb_")
}
```

### Step 3: Update trigger handlers in lib.rs

Find where `extract_pk` is called and update to pass entity:

```rust
// Before
let pk = extract_pk(&trigger)?;

// After - need to get entity from table name
let table_name = trigger.relation_name()?;
let entity = derive_entity_from_table(&table_name)
    .ok_or_else(|| TViewError::ConfigError {
        message: format!("Table '{}' does not follow tb_<entity> convention", table_name),
    })?;
let pk = extract_pk(&trigger, entity)?;
```

### Step 4: Handle edge cases

Add fallback for tables that don't follow naming convention:

```rust
/// Extract primary key with fallback strategies
///
/// 1. Try `pk_<entity>` based on table name convention
/// 2. Fall back to common PK column names: "id", "pk"
/// 3. Query pg_constraint for actual PK column
pub fn extract_pk_smart(trigger: &PgTrigger) -> spi::Result<i64> {
    let tuple = trigger
        .new()
        .or(trigger.old())
        .expect("Row must exist for AFTER trigger");

    let table_name = trigger.relation_name()?;

    // Strategy 1: Naming convention (tb_user -> pk_user)
    if let Some(entity) = derive_entity_from_table(&table_name) {
        let pk_column = format!("pk_{}", entity);
        if let Ok(Some(pk)) = tuple.get_by_name::<i64>(&pk_column) {
            return Ok(pk);
        }
    }

    // Strategy 2: Common PK column names
    for col in ["pk", "id"] {
        if let Ok(Some(pk)) = tuple.get_by_name::<i64>(col) {
            return Ok(pk);
        }
    }

    // Strategy 3: Would need to query pg_constraint - deferred
    Err(spi::Error::from(crate::TViewError::SpiError {
        query: format!("extract pk from {}", table_name),
        error: "Could not find primary key column".to_string(),
    }))
}
```

## Verification Commands

```bash
# Build check
cargo check --no-default-features --features pg18

# Run clippy
cargo clippy --no-default-features --features pg18 -- -D warnings

# Test with pgrx (if trigger tests exist)
cargo pgrx test pg18
```

## SQL Verification

After implementation:

```sql
-- Create tables following convention
CREATE TABLE tb_user (pk_user BIGINT PRIMARY KEY, name TEXT);
CREATE TABLE tb_post (pk_post BIGINT PRIMARY KEY, fk_user BIGINT, title TEXT);

-- Create TVIEWs
SELECT pg_tviews_create('user', 'SELECT pk_user, ...');
SELECT pg_tviews_create('post', 'SELECT pk_post, ...');

-- Test triggers fire correctly
INSERT INTO tb_user VALUES (1, 'Alice');  -- Should detect pk_user column
INSERT INTO tb_post VALUES (1, 1, 'Hi');  -- Should detect pk_post column

-- Both should work without hardcoding
```

## Acceptance Criteria

- [ ] `extract_pk()` accepts entity name parameter
- [ ] Entity name derived from table name automatically
- [ ] Works for any `tb_<entity>` table
- [ ] Clear error message when column not found
- [ ] Code compiles without warnings
- [ ] Clippy passes
- [ ] Existing functionality preserved

## DO NOT

- Do not add schema migrations to pg_tview_meta in this phase
- Do not query pg_constraint for PK detection (too slow for triggers)
- Do not break existing triggers
- Do not assume all tables have integer PKs (keep i64 for now, UUID support later)

## Future Enhancements

1. **Phase 4B**: Add `pk_column` to `pg_tview_meta` for explicit configuration
2. **Phase 4C**: Support composite primary keys
3. **Phase 4D**: Support UUID primary keys alongside integer PKs

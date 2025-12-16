# Phase 6: Workaround for TEXT[][] Array Extraction

## Objective

Document and implement a workaround for pgrx's lack of `TEXT[][]` (2D text array) support, enabling proper extraction of `dependency_paths` from `pg_tview_meta`.

## Context

Multiple locations have the same TODO:
- `src/catalog.rs:115`
- `src/catalog.rs:182`
- `src/lib.rs:712`

```rust
// TODO: pgrx doesn't support TEXT[][] extraction yet
// For now, use empty default (Task 3 will populate these)
let dep_paths: Vec<Option<Vec<String>>> = vec![];
```

The `dependency_paths` column in `pg_tview_meta` stores JSONB paths like:
- `{{"author"}, {"comments", "author"}}` - nested paths to dependent data

Currently, these are always returned as empty vectors, breaking nested JSONB refresh functionality.

## Root Cause

pgrx's `SpiHeapTupleData::value()` doesn't support extracting `TEXT[][]` (array of arrays). This is a known limitation of the pgrx library.

## Workaround Strategy

Instead of trying to extract `TEXT[][]` directly, we can:

### Option A: Store as JSONB instead of TEXT[][] (Recommended)

Change column type from `TEXT[][]` to `JSONB` which pgrx handles natively.

**Schema change:**
```sql
ALTER TABLE pg_tview_meta
    ALTER COLUMN dependency_paths TYPE JSONB
    USING to_jsonb(dependency_paths);
```

**Rust extraction:**
```rust
let dep_paths_jsonb: Option<JsonB> = row["dependency_paths"].value()?;
let dep_paths: Vec<Option<Vec<String>>> = dep_paths_jsonb
    .map(|j| serde_json::from_value(j.0).unwrap_or_default())
    .unwrap_or_default();
```

### Option B: Use JSON string encoding in TEXT[]

Store as `TEXT[]` with JSON-encoded strings:
- `['["author"]', '["comments", "author"]']`

**Rust extraction:**
```rust
let dep_paths_raw: Option<Vec<String>> = row["dependency_paths"].value()?;
let dep_paths: Vec<Option<Vec<String>>> = dep_paths_raw
    .unwrap_or_default()
    .into_iter()
    .map(|s| serde_json::from_str(&s).ok())
    .collect();
```

### Option C: Use custom SPI query

Bypass pgrx's type handling with raw SPI:
```rust
let dep_paths = Spi::get_one::<String>(&format!(
    "SELECT array_to_json(dependency_paths)::text FROM pg_tview_meta WHERE entity = '{}'",
    entity
))?;
let parsed: Vec<Vec<String>> = serde_json::from_str(&dep_paths.unwrap_or_default())?;
```

## Recommendation

**Option A (JSONB column)** is cleanest long-term but requires schema migration.

**Option C (SPI workaround)** works without schema changes and is safest for existing installations.

## Files to Modify

| File | Changes |
|------|---------|
| `src/catalog.rs` | Implement workaround in `load_for_source()` and `load_for_entity()` |
| `src/lib.rs` | Update any direct metadata loading |
| `sql/migrations/` | Optional: schema migration for Option A |

## Implementation Steps (Option C - No Schema Change)

### Step 1: Add helper function for 2D array extraction

```rust
// In src/catalog.rs

/// Extract TEXT[][] as Vec<Option<Vec<String>>> using JSON conversion
///
/// Workaround for pgrx not supporting TEXT[][] extraction.
/// Converts via PostgreSQL's array_to_json() function.
fn extract_text_2d_array(
    entity: &str,
    column: &str,
) -> TViewResult<Vec<Option<Vec<String>>>> {
    let query = format!(
        "SELECT COALESCE(array_to_json({})::text, '[]') FROM pg_tview_meta WHERE entity = $1",
        column
    );

    let args = vec![unsafe {
        DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
    }];

    let json_str = Spi::get_one_with_args::<String>(&query, &args)?
        .unwrap_or_else(|| "[]".to_string());

    // Parse JSON array of arrays: [["path1"], ["path2", "subpath"], null]
    let parsed: Vec<Option<Vec<String>>> = serde_json::from_str(&json_str)
        .map_err(|e| crate::TViewError::SpiError {
            query: query.clone(),
            error: format!("Failed to parse dependency_paths JSON: {}", e),
        })?;

    Ok(parsed)
}
```

### Step 2: Update load_for_source()

```rust
// In TviewMeta::load_for_source()

// Replace:
let dep_paths: Vec<Option<Vec<String>>> = vec![];

// With:
let entity_name: String = row["entity"].value()?
    .ok_or_else(|| /* error */)?;
let dep_paths = extract_text_2d_array(&entity_name, "dependency_paths")
    .unwrap_or_default();
```

### Step 3: Update load_for_entity()

Same pattern as Step 2.

### Step 4: Update from_spi_row()

Same pattern, but entity is already known.

## Verification Commands

```bash
# Build check
cargo check --no-default-features --features pg18

# Run clippy
cargo clippy --no-default-features --features pg18 -- -D warnings

# Test extraction
cargo pgrx test pg18
```

## SQL Verification

```sql
-- Insert test metadata with dependency paths
INSERT INTO pg_tview_meta (entity, table_oid, view_oid, dependency_paths)
VALUES ('test', 12345, 67890, ARRAY[ARRAY['author'], ARRAY['comments', 'author']]);

-- Verify extraction works via function
SELECT pg_tviews_metadata_info('test');
-- Should show dependency_paths correctly
```

## Acceptance Criteria

- [ ] `dependency_paths` extracted correctly from metadata
- [ ] Nested JSONB refresh uses correct paths
- [ ] No schema migration required (Option C)
- [ ] Backward compatible with existing data
- [ ] Code compiles without warnings
- [ ] Clippy passes

## DO NOT

- Do not modify the pg_tview_meta schema (for Option C)
- Do not add new dependencies beyond serde_json
- Do not break existing metadata loading
- Do not change the TviewMeta struct fields

## Future Work

Consider migrating to JSONB column type in a major version release:
- Cleaner extraction without workaround
- Native pgrx support
- Better validation capabilities

## Error Handling

If extraction fails, fall back to empty paths (current behavior):
```rust
let dep_paths = extract_text_2d_array(&entity_name, "dependency_paths")
    .unwrap_or_else(|e| {
        warning!("Failed to extract dependency_paths: {}", e);
        vec![]
    });
```

This ensures existing functionality isn't broken even if the workaround has issues.

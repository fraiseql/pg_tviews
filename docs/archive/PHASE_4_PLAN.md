# Phase 4 Implementation Plan: Refresh & Cascade Logic

**Status:** Ready to implement
**Duration:** 14-21 days
**Complexity:** CRITICAL (Core value proposition)
**Prerequisites:** ✅ Phases 0-3 complete

---

## Current Status

### ✅ What's Already Working (Phases 0-3)

1. **Extension Foundation** (Phase 0)
   - Extension compiles and loads
   - Metadata tables created (`pg_tview_meta`)
   - Error types defined
   - Version function working

2. **Schema Inference** (Phase 1)
   - Column detection (pk_, id, fk_*, *_id, data)
   - Type inference from catalog
   - `pg_tviews_analyze_select()` function

3. **DDL & Tables** (Phase 2)
   - `CREATE TVIEW tv_X AS SELECT ...` syntax
   - Backing view `v_X` created
   - Materialized table `tv_X` with schema
   - Initial data population
   - `DROP TVIEW` cleanup

4. **Dependency & Triggers** (Phase 3)
   - Recursive pg_depend graph traversal
   - Detects transitive dependencies through helpers
   - Installs AFTER triggers on base tables
   - Trigger handler stubs (logs only, no refresh yet)

### ⏳ What Needs Implementation (Phase 4)

**Current Gaps:**

1. **src/refresh.rs** - Has skeleton but needs:
   - Real FK column extraction from view rows
   - Integration with jsonb_ivm for surgical updates
   - Proper error handling
   - Transaction isolation checks

2. **src/propagate.rs** - Just TODOs:
   - Find parent entities via pg_depend
   - FK lineage tracking (fk_user = X → find all posts)
   - Cascade propagation logic
   - Depth limiting

3. **Trigger Handler** - In `src/dependency/triggers.rs`:
   - Currently just logs
   - Needs dynamic PK extraction
   - FK change detection on UPDATE
   - Call to Rust cascade function

4. **Concurrency** - Not started:
   - Advisory locks for row-level safety
   - Transaction isolation enforcement
   - Deadlock prevention

---

## Implementation Tasks (RED → GREEN → REFACTOR)

### Task 1: Fix Trigger Handler (Dynamic PK Extraction) ⚠️ CRITICAL FIX

**Current Problem:** Trigger handler doesn't extract PK dynamically.

**Solution:**
```sql
-- In create_trigger_handler() (src/dependency/triggers.rs)

-- Get PK column name for changed table
SELECT a.attname INTO pk_col_name
FROM pg_index i
JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
WHERE i.indrelid = TG_RELID AND i.indisprimary
LIMIT 1;

-- Extract PK values dynamically
EXECUTE format('SELECT ($1).%I', pk_col_name) USING OLD INTO pk_val_old;
EXECUTE format('SELECT ($1).%I', pk_col_name) USING NEW INTO pk_val_new;
```

**Files to edit:**
- `src/dependency/triggers.rs` - Replace create_trigger_handler() function

**Test:**
```sql
-- test/sql/40_refresh_trigger_dynamic_pk.sql
CREATE TABLE tb_post (pk_post INT PRIMARY KEY);
CREATE TVIEW tv_post AS SELECT pk_post, ... FROM tb_post;
UPDATE tb_post SET ... WHERE pk_post = 1; -- Should extract pk_post correctly
```

---

### Task 2: Implement Single Row Refresh

**Goal:** Recompute one row from v_* and update tv_* with new data.

**Implementation:**

1. **Extract FK columns from view row** (`src/refresh.rs`):
```rust
fn extract_fk_columns(
    row: &SpiTupleTable,
    meta: &TviewMeta,
) -> TViewResult<Vec<(String, i64)>> {
    let mut fk_values = Vec::new();

    for fk_col in &meta.fk_columns {
        if let Some(val) = row[fk_col.as_str()].value::<i64>()? {
            fk_values.push((fk_col.clone(), val));
        }
    }

    Ok(fk_values)
}
```

2. **Update recompute_view_row()** to populate fk_values:
```rust
// In recompute_view_row()
let data: JsonB = row["data"].value()?.unwrap();
let fk_values = extract_fk_columns(&row, meta)?;
let uuid_fk_values = extract_uuid_fk_columns(&row, meta)?;

Ok(ViewRow {
    entity_name: meta.entity_name.clone(),
    pk,
    tview_oid: meta.tview_oid,
    view_oid: meta.view_oid,
    data,
    fk_values,       // Now populated!
    uuid_fk_values,  // Now populated!
})
```

**Test:**
```sql
-- test/sql/41_refresh_single_row.sql
BEGIN;
    SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

    CREATE TABLE tb_post (
        pk_post INT PRIMARY KEY,
        id UUID,
        title TEXT
    );

    INSERT INTO tb_post VALUES (1, gen_random_uuid(), 'Original');

    CREATE TVIEW tv_post AS
    SELECT pk_post, id, jsonb_build_object('id', id, 'title', title) AS data
    FROM tb_post;

    -- Verify initial
    SELECT data->>'title' FROM tv_post; -- 'Original'

    -- Update
    UPDATE tb_post SET title = 'Updated' WHERE pk_post = 1;

    -- Verify refresh happened
    SELECT data->>'title' FROM tv_post; -- 'Updated'

ROLLBACK;
```

---

### Task 3: Implement FK Lineage Cascade

**Goal:** When tv_user row changes, find and refresh all tv_post rows where fk_user = X.

**Implementation:**

1. **Find parent entities** (`src/propagate.rs`):
```rust
pub fn find_parent_entities(entity: &str) -> TViewResult<Vec<String>> {
    let query = format!(
        "SELECT DISTINCT m.entity
         FROM pg_tview_meta m
         CROSS JOIN unnest(m.dependencies) AS dep(oid)
         WHERE dep = (
             SELECT view_oid FROM pg_tview_meta WHERE entity = '{}'
         )",
        entity
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, None)?;
        let mut parents = Vec::new();
        for row in rows {
            if let Some(parent) = row["entity"].value::<String>()? {
                parents.push(parent);
            }
        }
        Ok(Some(parents))
    })?
    .ok_or_else(|| TViewError::SpiError {
        query,
        error: "SPI failed".to_string(),
    })
}
```

2. **Find affected PKs** (`src/propagate.rs`):
```rust
pub fn find_affected_pks(
    parent_entity: &str,
    child_entity: &str,
    child_pk: i64,
) -> TViewResult<Vec<i64>> {
    // Find FK column that points from parent to child
    // E.g., tv_post has fk_user pointing to tv_user

    let fk_col = format!("fk_{}", child_entity);
    let parent_table = format!("tv_{}", parent_entity);
    let parent_pk_col = format!("pk_{}", parent_entity);

    let query = format!(
        "SELECT {} FROM {} WHERE {} = {}",
        parent_pk_col, parent_table, fk_col, child_pk
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, None)?;
        let mut pks = Vec::new();
        for row in rows {
            if let Some(pk) = row[parent_pk_col.as_str()].value::<i64>()? {
                pks.push(pk);
            }
        }
        Ok(Some(pks))
    })?
    .ok_or_else(|| TViewError::SpiError {
        query,
        error: "SPI failed".to_string(),
    })
}
```

3. **Update propagate_from_row()** to use these:
```rust
pub fn propagate_from_row(row: &ViewRow) -> TViewResult<()> {
    // Find all parent entities that depend on this entity
    let parents = find_parent_entities(&row.entity_name)?;

    for parent in parents {
        // Find all rows in parent TVIEW affected by this row
        let affected_pks = find_affected_pks(&parent, &row.entity_name, row.pk)?;

        info!("Cascading {} change to {} rows in tv_{}",
              row.entity_name, affected_pks.len(), parent);

        for pk in affected_pks {
            // Refresh each affected parent row
            crate::refresh::refresh_tview_row(&parent, pk)?;
        }
    }

    Ok(())
}
```

**Test:**
```sql
-- test/sql/42_cascade_fk_lineage.sql
BEGIN;
    SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

    CREATE TABLE tb_user (pk_user INT PRIMARY KEY, id UUID, name TEXT);
    CREATE TABLE tb_post (
        pk_post INT PRIMARY KEY,
        id UUID,
        fk_user INT,
        title TEXT
    );

    INSERT INTO tb_user VALUES (1, gen_random_uuid(), 'Alice');
    INSERT INTO tb_post VALUES (10, gen_random_uuid(), 1, 'Post 1');
    INSERT INTO tb_post VALUES (11, gen_random_uuid(), 1, 'Post 2');

    CREATE TVIEW tv_user AS
    SELECT pk_user, id, jsonb_build_object('id', id, 'name', name) AS data
    FROM tb_user;

    CREATE TVIEW tv_post AS
    SELECT p.pk_post, p.id, p.fk_user, u.id AS user_id,
           jsonb_build_object(
               'id', p.id,
               'title', p.title,
               'author', v_user.data
           ) AS data
    FROM tb_post p
    JOIN v_user ON v_user.pk_user = p.fk_user;

    -- Update user name
    UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1;

    -- Verify cascade: both posts should have updated author.name
    SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 10;
    -- Expected: 'Alice Updated'

    SELECT data->'author'->>'name' FROM tv_post WHERE pk_post = 11;
    -- Expected: 'Alice Updated'

ROLLBACK;
```

---

### Task 4: Add Cascade Depth Limiting

**Goal:** Prevent infinite loops in cascade propagation.

**Implementation:**

1. **Add depth tracking** (`src/refresh.rs`):
```rust
pub const MAX_CASCADE_DEPTH: usize = 10;

pub fn refresh_tview_row_with_depth(
    entity: &str,
    pk_value: i64,
    depth: usize,
) -> TViewResult<()> {
    if depth >= MAX_CASCADE_DEPTH {
        return Err(TViewError::CascadeDepthExceeded {
            current_depth: depth,
            max_depth: MAX_CASCADE_DEPTH,
        });
    }

    // ... rest of refresh logic ...

    // Pass depth+1 to propagate
    propagate_from_row_with_depth(&row, depth + 1)?;

    Ok(())
}
```

2. **Update error types** (`src/error/mod.rs`):
```rust
#[derive(Debug, Clone)]
pub enum TViewError {
    // ... existing variants ...

    CascadeDepthExceeded {
        current_depth: usize,
        max_depth: usize,
    },
}
```

**Test:**
```sql
-- test/sql/43_cascade_depth_limit.sql
-- Create circular dependency (should hit depth limit)
BEGIN;
    -- This would create infinite cascade
    -- Should fail with CascadeDepthExceeded error
ROLLBACK;
```

---

### Task 5: Add Transaction Isolation Check

**Goal:** Enforce REPEATABLE READ or SERIALIZABLE isolation.

**Implementation:**

Update trigger handler:
```sql
-- In create_trigger_handler()
BEGIN
    -- Check transaction isolation
    IF current_setting('transaction_isolation') NOT IN ('repeatable read', 'serializable') THEN
        RAISE WARNING 'pg_tviews requires REPEATABLE READ isolation. Current: %',
            current_setting('transaction_isolation');
        -- Don't fail, just warn for now
    END IF;

    -- ... rest of trigger logic ...
END;
```

**Documentation:**
- Create `docs/CONCURRENCY.md` explaining isolation requirements
- Update README with `ALTER DATABASE` recommendation

---

### Task 6: Wire Trigger to Rust Cascade Function

**Goal:** Make trigger actually call Rust code for refresh.

**Implementation:**

1. **Export cascade function** (`src/lib.rs`):
```rust
#[pg_extern]
fn pg_tviews_cascade(
    source_table_oid: pg_sys::Oid,
    pk_new: Option<i64>,
    pk_old: Option<i64>,
    cascade_depth: i32,
) -> TViewResult<()> {
    // Handle INSERT/UPDATE
    if let Some(pk) = pk_new {
        refresh_pk_with_depth(source_table_oid, pk, cascade_depth as usize)?;
    }

    // Handle DELETE (or UPDATE FK change)
    if let Some(pk) = pk_old {
        if Some(pk) != pk_new {
            refresh_pk_with_depth(source_table_oid, pk, cascade_depth as usize)?;
        }
    }

    Ok(())
}
```

2. **Update trigger handler** to call it:
```sql
-- In create_trigger_handler()
PERFORM pg_tviews_cascade(TG_RELID, pk_val_new, pk_val_old, cascade_depth + 1);
```

**Test:**
```sql
-- test/sql/44_trigger_cascade_integration.sql
-- Full end-to-end: base table change → trigger → cascade → verify
```

---

## Acceptance Criteria

### Functional

- [ ] Single row refresh works (SELECT FROM v_*, UPDATE tv_*)
- [ ] FK columns extracted from view rows
- [ ] FK lineage cascade works (parent entities refreshed)
- [ ] Multi-level cascade (A → B → C)
- [ ] INSERT/UPDATE/DELETE all trigger refresh
- [ ] Cascade depth limited to 10
- [ ] Transaction isolation checked (warning if not REPEATABLE READ)
- [ ] updated_at timestamp maintained

### Quality

- [ ] All Rust unit tests pass
- [ ] SQL integration tests pass (40-44)
- [ ] Error handling comprehensive
- [ ] Clear error messages
- [ ] No panics in error cases

### Performance

- [ ] Single row refresh < 5ms
- [ ] 100-row cascade < 500ms
- [ ] No memory leaks in large cascades

---

## Files to Modify

### Primary Changes

1. **src/refresh.rs**
   - Extract FK columns from view rows
   - Add depth parameter to refresh_tview_row()
   - Implement refresh_tview_row_with_depth()

2. **src/propagate.rs**
   - Implement find_parent_entities()
   - Implement find_affected_pks()
   - Implement propagate_from_row_with_depth()

3. **src/dependency/triggers.rs**
   - Fix create_trigger_handler() with dynamic PK extraction
   - Add FK change detection
   - Call pg_tviews_cascade()

4. **src/lib.rs**
   - Export pg_tviews_cascade()

5. **src/error/mod.rs**
   - Add CascadeDepthExceeded variant

### New Files

6. **test/sql/40_refresh_trigger_dynamic_pk.sql**
7. **test/sql/41_refresh_single_row.sql**
8. **test/sql/42_cascade_fk_lineage.sql**
9. **test/sql/43_cascade_depth_limit.sql**
10. **test/sql/44_trigger_cascade_integration.sql**
11. **docs/CONCURRENCY.md** (optional but recommended)

---

## Testing Strategy

### Unit Tests (Rust)

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_extract_fk_columns() {
        // Test FK extraction from SPI row
    }

    #[test]
    fn test_cascade_depth_limit() {
        // Should fail at depth 10
    }
}
```

### Integration Tests (SQL)

**Test progression:**
1. Single table, single row refresh
2. Two tables with FK, cascade from child to parent
3. Three-level hierarchy (company → user → post)
4. Depth limit enforcement
5. Full end-to-end with triggers

---

## Rollback Plan

If Phase 4 fails or has critical issues:

1. **Disable refresh in triggers** - Keep triggers but make them no-ops
2. **Document manual refresh** - Provide SQL functions for manual refresh
3. **Fix and retry** - Address specific issues and re-run affected tests

Can rollback to Phase 3 state (triggers installed but inactive).

---

## Timeline Estimate

| Task | Duration | Dependencies |
|------|----------|--------------|
| 1. Fix trigger handler | 1 day | None |
| 2. Single row refresh | 2-3 days | Task 1 |
| 3. FK lineage cascade | 3-4 days | Task 2 |
| 4. Cascade depth limiting | 1-2 days | Task 3 |
| 5. Isolation check | 1 day | None (parallel) |
| 6. Wire trigger to Rust | 2 days | Tasks 1-4 |
| 7. Testing & debugging | 4-7 days | All tasks |

**Total: 14-21 days**

---

## Success Metrics

**Phase 4 is complete when:**

✅ All 5 SQL integration tests pass
✅ `cargo test` passes
✅ Real PrintOptim-like scenario works (company → user → post cascade)
✅ Performance target met (< 500ms for 100-row cascade)
✅ No memory leaks in stress test (1000 row cascade)
✅ Documentation complete

---

## Next Steps

After Phase 4 completion:
- **Phase 5:** Array handling & jsonb_ivm optimization
- **Production readiness:** Monitoring, logging, error telemetry

---

## Notes

- **MOST CRITICAL PHASE** - This brings pg_tviews to life!
- Test with realistic PrintOptim schemas (company/user/project/task)
- Performance benchmarking is essential
- Transaction isolation requirement must be documented prominently
- Consider adding DEBUG_REFRESH flag for verbose logging during development

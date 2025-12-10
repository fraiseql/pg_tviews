# Phase 5 Task 7: Implement Array Handling (GREEN Phase)

**Status:** READY TO START
**Prerequisites:** Phase 5 Task 6.2 (Documentation Corrected)
**TDD Phase:** GREEN (tests already written in RED phase)
**Estimated Time:** [X] days

## Objective

Implement the array handling functionality that was designed and tested in Phase 5,
making tests 50-52 pass.

## Tests to Pass (RED Phase Complete)

From `test/sql/`:
- `50_array_columns.sql` - Array column materialization
- `51_jsonb_array_update.sql` - JSONB array element updates
- `52_array_insert_delete.sql` - Array INSERT/DELETE operations

## Implementation Tasks

### Task 1: Fix Missing Trigger Handler
**File:** `src/trigger.rs`
- [ ] Implement `pg_tview_trigger_handler_wrapper` function
- [ ] Expose function with `#[pg_extern]` attribute
- [ ] Ensure extension can load properly

### Task 2: Schema Inference for Arrays
**File:** `src/schema/inference.rs`
- [ ] Detect `ARRAY(...)` patterns in SQL
- [ ] Infer array element types (UUID[], TEXT[], etc.)
- [ ] Store array column metadata

### Task 3: Array Element INSERT
**File:** `src/refresh/array_ops.rs`
- [ ] Implement `insert_array_element()` function
- [ ] Handle JSONB array append
- [ ] Handle SQL array append

### Task 4: Array Element DELETE
**File:** `src/refresh/array_ops.rs`
- [ ] Implement `delete_array_element()` function
- [ ] Handle JSONB array element removal
- [ ] Handle SQL array element removal

### Task 5: Batch Optimization
**File:** `src/refresh/batch.rs`
- [ ] Implement threshold detection (10 rows)
- [ ] Implement batch refresh logic
- [ ] Integrate with array operations

## Verification

After implementation:
1. Run tests: `cargo pgrx test pg17 --no-default-features --features pg17 -- --test-threads=1`
2. Verify tests 50-52 pass
3. Run performance benchmarks
4. Document actual results
5. Update documentation with verified metrics

## Success Criteria

- ✅ Tests 50-52 all pass
- ✅ Performance ≥ 2.0× improvement
- ✅ Batch optimization working
- ✅ Results documented
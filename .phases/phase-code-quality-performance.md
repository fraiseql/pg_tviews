# Phase: Code Quality & Performance

## Objective

Fix critical code quality bugs, reduce duplication, improve module organization, and add missing indexes for cascade performance at scale.

## Current State (2026-03-19)

### Critical Bugs

1. **Wrong SQLSTATE in error conversion** — `src/error/mod.rs:356-363`: `From<TViewError> for pgrx::spi::Error` always returns `InvalidPosition` regardless of actual error type. Client applications receive wrong error codes.

2. **Silent error swallowing** — `src/lib.rs:597-600`: SPI failures return empty Vec with no logging, making bugs invisible.

### Code Quality Issues

3. **Metadata parsing duplicated 4x** — `catalog.rs` has `load_for_source()`, `load_by_entity()`, `from_spi_row()`, and `lib.rs:find_dependent_tviews()` all parsing the same SPI rows independently.

4. **God file** — `src/lib.rs` at 1,173 lines mixes initialization, health checks, 2PC handling, cascades, and tests.

5. **Clippy pedantic not enabled** — `Cargo.toml` only has `all = deny`, missing `pedantic = deny` per project standards.

6. **Unused Serialize/Deserialize derives** — `TviewMeta` derives serde traits but is never serialized.

7. **Inconsistent error handling** — Mix of `?`, `.map_err()`, `.unwrap_or_else()`, `.ok()` patterns across modules.

### Performance Issues

8. **Missing indexes on `pg_tview_meta`** — No indexes on `entity`, `fk_columns`, or `dependencies`. Every catalog lookup is a sequential scan. Becomes bottleneck at 50+ TVIEWs.

9. **N+1 parent discovery** — `propagate.rs:62-84` queries `pg_tview_meta` once per FK column per changed row during cascade.

10. **Graph cache never auto-invalidated** — DDL operations (CREATE/DROP TVIEW) don't call `invalidate_all_caches()`, so the dependency graph can go stale until server restart.

11. **Bulk refresh selects all columns** — `refresh/bulk.rs:61` does `SELECT *` from wide views instead of `(pk, data)`.

---

## TDD Cycles

### Cycle 1: Fix error conversion bug (Critical)

**RED**: Write a test that creates a `TViewError::MetadataNotFound` and converts it via `Into<pgrx::spi::Error>`. Assert it does NOT produce `InvalidPosition`.

**GREEN**: Rewrite the `From<TViewError> for pgrx::spi::Error` impl to map each variant to appropriate SPI error types, or remove the impl entirely if it's unused (check callers first).

**REFACTOR**: If the impl exists only to satisfy a trait bound that's never exercised, delete it.
**CLEANUP**: Verify error codes in psql: `DO $$ BEGIN PERFORM pg_tviews_refresh('nonexistent'); END $$;` should return meaningful SQLSTATE.

### Cycle 2: Consolidate metadata parsing

**RED**: Existing tests must continue to pass after refactor.

**GREEN**: Make `load_for_source()` and `load_by_entity()` both delegate to `from_spi_row()` for row parsing. Remove duplicated field extraction from `lib.rs:find_dependent_tviews()` — have it call `TviewMeta::from_spi_row()` instead.

**REFACTOR**: Consider making `from_spi_row()` the single constructor, with `load_*` methods as thin query + parse wrappers.
**CLEANUP**: Verify no behavior change. Remove dead parsing code.

### Cycle 3: Add pg_tview_meta indexes

**RED**: Benchmark `find_parents_for()` with 50+ TVIEWs registered. Measure current query time.

**GREEN**: Add indexes in the extension's SQL setup:
```sql
CREATE INDEX IF NOT EXISTS idx_pg_tview_meta_entity
  ON pg_tview_meta(entity);
CREATE INDEX IF NOT EXISTS idx_pg_tview_meta_table_oid
  ON pg_tview_meta(table_oid);
CREATE INDEX IF NOT EXISTS idx_pg_tview_meta_fk_columns
  ON pg_tview_meta USING GIN(fk_columns);
```

**REFACTOR**: Verify indexes are created in the extension's `CREATE EXTENSION` path (check SQL install scripts).
**CLEANUP**: Re-benchmark. Document improvement.

### Cycle 4: Wire cache invalidation to DDL

**RED**: Test sequence: CREATE TVIEW A, CREATE TVIEW B (depends on A), modify A's base table — verify B refreshes. Currently may fail if graph cache was populated before B was created.

**GREEN**: Call `invalidate_all_caches()` from `pg_tviews_create()` and `pg_tviews_drop()` after metadata changes. Locate these in `src/ddl/create.rs` and `src/ddl/drop.rs` (or equivalent).

**REFACTOR**: Consider fine-grained invalidation (only invalidate affected entity subgraph) if cache rebuild is expensive.
**CLEANUP**: Add integration test for the sequence above.

### Cycle 5: Eliminate silent error swallowing

**RED**: Trigger an SPI error in the code path at `lib.rs:597-600`. Verify it's currently invisible (no log entry).

**GREEN**: Replace `.unwrap_or_else(|_e| Vec::new())` with proper logging:
```rust
.unwrap_or_else(|e| {
    warning!("Failed to load dependent TVIEWs: {e}");
    Vec::new()
})
```

Apply the same pattern to all `.ok()` calls that discard errors — add `warning!()` or convert to `?`.

**REFACTOR**: Audit all `.ok()`, `.unwrap_or()`, `.unwrap_or_else()` calls. Classify each as:
  - Intentionally ignored (add `// Intentional: <reason>` comment)
  - Should propagate (convert to `?`)
  - Should log (add `warning!()`)

**CLEANUP**: Zero unexplained `.ok()` calls remaining.

### Cycle 6: Enable clippy::pedantic

**RED**: Run `cargo clippy --no-default-features --features pg18 -- -W clippy::pedantic`. Capture all new warnings.

**GREEN**: Add `pedantic = { level = "deny", priority = -1 }` to `[lints.clippy]` in Cargo.toml. Fix all resulting warnings. Expected categories:
  - `doc_markdown` — backtick identifiers in doc comments (~30 fixes, overlaps with CI Health phase)
  - `cast_possible_truncation` — audit integer casts
  - `cast_sign_loss` — audit unsigned conversions
  - `needless_pass_by_value` — take `&str` instead of `String` where appropriate
  - `must_use_candidate` — add `#[must_use]` to pure functions

**REFACTOR**: Add targeted `#[allow()]` with `// Reason:` comments only where pedantic is genuinely wrong for pgrx FFI code.
**CLEANUP**: CI clippy passes with pedantic enabled.

### Cycle 7: Split lib.rs into focused modules

**RED**: All existing tests must pass after the split.

**GREEN**: Extract from `src/lib.rs`:
  - `src/lifecycle.rs` — `_PG_init()`, hook registration, extension metadata
  - `src/health.rs` — `pg_tviews_health_check()`, diagnostics
  - `src/twophase.rs` — `pg_tviews_prepare()`, `pg_tviews_commit_prepared()`, `pg_tviews_rollback_prepared()`
  - Keep `src/lib.rs` as thin re-export hub

**REFACTOR**: Move inline tests to `tests/` directory or `#[cfg(test)]` submodules in each new file.
**CLEANUP**: `src/lib.rs` should be under 200 lines. All imports updated.

### Cycle 8: Optimize bulk refresh column selection

**RED**: Benchmark bulk refresh on a TVIEW with 20+ columns. Measure bytes transferred.

**GREEN**: In `src/refresh/bulk.rs`, replace `SELECT *` with `SELECT {pk_col}, {data_col}` where `data_col` is the JSONB column. Fall back to `SELECT *` only if the TVIEW uses non-standard column layout.

**REFACTOR**: Extract column list building into a helper shared with single-row refresh.
**CLEANUP**: Re-benchmark. Verify no behavior change for standard and non-standard TVIEWs.

### Cycle 9: Remove dead code and unused derives

**RED**: `cargo clippy` with `dead_code` warnings un-suppressed.

**GREEN**:
  - Remove `Serialize`/`Deserialize` from `TviewMeta` if unused
  - Remove `#[allow(dead_code)]` from `get_prepared_transaction_id()` if it's actually used, or delete the function if truly dead
  - Audit `src/refresh/array_ops.rs` — if all functions are `#[allow(dead_code)]`, either implement the feature or remove the module
  - Remove `sync_mode` field or its "async future" comment if async is not planned

**CLEANUP**: Zero `#[allow(dead_code)]` without justification.

---

## Dependencies

- Cycle 6 (pedantic) overlaps with CI Health Phase Cycle 2 (doc_markdown fixes) — do whichever lands first, the other becomes a no-op
- Cycle 3 (indexes) can be done independently at any time
- Cycle 7 (split lib.rs) should come after Cycles 1-5 to avoid merge conflicts

## Priority Order

1. **Cycle 1** — Error conversion bug (correctness, affects all clients)
2. **Cycle 3** — Indexes (performance, no code risk)
3. **Cycle 4** — Cache invalidation (correctness at scale)
4. **Cycle 5** — Error swallowing (debuggability)
5. **Cycle 2** — Metadata dedup (maintainability)
6. **Cycle 6** — Pedantic clippy (code quality gate)
7. **Cycle 8** — Bulk refresh optimization (performance)
8. **Cycle 7** — Module split (organization, large diff)
9. **Cycle 9** — Dead code cleanup (hygiene)

## Status
[x] Cycle 1: Fix error conversion (From<TViewError> for spi::Error → OpUnknown instead of InvalidPosition)
[x] Cycle 3: Add pg_tview_meta index (table_oid)
[x] Cycle 4: Cache invalidation already wired (create.rs:117, drop.rs:72)
[x] Cycle 5: Fix silent error swallowing (warning! instead of silent empty vec)
[x] Cycle 6: Pedantic clippy (done in CI Health phase)
[x] Cycle 2: Metadata dedup (from_spi_row is single constructor, load_* are thin wrappers)
[x] Cycle 7: Split lib.rs (187 lines: lifecycle, health, twophase, cascade, admin modules)
[x] Cycle 8: Bulk refresh optimization (SELECT pk+data instead of SELECT *)
[x] Cycle 9: Dead code cleanup (serde derives removed, sync_mode removed, all dead_code justified)

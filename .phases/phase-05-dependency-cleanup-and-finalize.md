# Phase 5: Dependency Cleanup & Finalize

## Objective

Remove unused crate dependencies to reduce compile times and attack surface.
Perform a final verification pass ensuring all prior phases are complete and
the codebase is clean.

## Why This Matters

Every unused dependency adds compile time, binary size, and potential supply-chain
risk. `flate2` appears completely unused. `bincode` is only used for a `From` impl
that may never be triggered. `chrono` features may be unnecessary.

## Success Criteria

- [ ] `flate2` removed from `Cargo.toml`
- [ ] `bincode` assessed and removed if unused in practice
- [ ] `chrono` assessed and removed if unused in practice
- [ ] `cargo clippy --no-default-features --features pg18` clean
- [ ] `cargo test` passes
- [ ] `cargo build --release --no-default-features --features pg18` succeeds
- [ ] No `TODO`, `FIXME`, `HACK` comments remain (finalization check)
- [ ] No references to `.phases/` in shipped code

## Files To Change

### 1. `Cargo.toml` — Remove `flate2`

```toml
# REMOVE this line:
flate2 = "1.0"
```

Verify no code imports it:
```bash
grep -rn 'flate2\|GzEncoder\|GzDecoder\|Compression' src/ --include='*.rs'
```

### 2. `Cargo.toml` — Assess `bincode`

Check usage:
```bash
grep -rn 'bincode' src/ --include='*.rs'
```

Expected results:
- `error/mod.rs`: `impl From<bincode::Error> for TViewError` (line ~369)

If this is the only usage and no code path actually produces a `bincode::Error`,
remove:
- The `From<bincode::Error>` impl from `error/mod.rs`
- The `bincode = "1.3"` line from `Cargo.toml`

If bincode is used elsewhere (e.g., in serialization for 2PC queue persistence
that was planned but not implemented), remove it and re-add when actually needed.

### 3. `Cargo.toml` — Assess `chrono`

Check usage:
```bash
grep -rn 'chrono\|NaiveDate\|DateTime\|Utc' src/ --include='*.rs'
```

If the only usage is `chrono = { version = "0.4", features = ["serde"] }` in
Cargo.toml with no actual imports in source, remove it.

If `chrono` types are used in serialization or `RefreshKey` (check `serde` derives
that might use chrono features), keep it.

### 4. Final Quality Pass

Run through the finalization checklist from CLAUDE.md:

```bash
# 1. All tests pass
cargo test

# 2. All lints pass (zero warnings)
cargo clippy --no-default-features --features pg18 -- -D warnings

# 3. Build succeeds in release mode
cargo build --release --no-default-features --features pg18

# 4. No TODO/FIXME remaining
grep -rn 'TODO\|FIXME\|HACK\|XXX' src/ --include='*.rs' | grep -v 'test\|#\[allow'

# 5. No phase references in code
grep -rn 'Phase\|phase' src/ --include='*.rs' | grep -v 'test\|#\[allow'

# 6. No dead_code warnings (assess remaining #[allow(dead_code)] annotations)
grep -rn 'allow(dead_code)' src/ --include='*.rs'
# Review each: is the justification still valid?
```

## TDD Cycles

### Cycle 1: Remove `flate2`

- **RED**: `cargo build --no-default-features --features pg18` succeeds
- **GREEN**: Remove `flate2` from `Cargo.toml`.
- **REFACTOR**: None
- **CLEANUP**: `cargo build`, verify no compile errors

### Cycle 2: Assess and remove `bincode`

- **RED**: `cargo test` passes
- **GREEN**: If bincode is unused in practice, remove the `From<bincode::Error>` impl
  and the Cargo.toml dependency.
- **REFACTOR**: None
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 3: Assess and remove `chrono`

- **RED**: `cargo test` passes
- **GREEN**: If chrono is unused, remove from Cargo.toml.
- **REFACTOR**: None
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 4: Final verification

- **RED**: Run full checklist above
- **GREEN**: Fix any remaining issues found
- **REFACTOR**: None
- **CLEANUP**: Final `cargo clippy`, `cargo test`, `cargo build --release`

## Verification

```bash
# Full clean build:
cargo clean && cargo build --no-default-features --features pg18

# Clippy:
cargo clippy --no-default-features --features pg18 -- -D warnings

# Tests:
cargo test

# Release build:
cargo build --release --no-default-features --features pg18

# Check binary size reduction (before vs after):
ls -la target/release/libpg_tviews.so 2>/dev/null || \
ls -la target/release/pg_tviews.so 2>/dev/null || \
echo "Check target/ for the .so file"
```

## Dependencies

- Requires: Phases 1-4 complete
- Blocks: Nothing (final phase)

## Status
[ ] Not Started

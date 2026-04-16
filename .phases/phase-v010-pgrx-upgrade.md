# Phase 1: pgrx 0.16.1 → 0.17.0 Upgrade

## Objective
Upgrade pgrx dependency to match pg_treekey's pinned version for ABI compatibility.

## Success Criteria
- [ ] Cargo.toml pins pgrx/pgrx-tests to =0.17.0
- [ ] Edition 2024 / resolver 3
- [ ] All DatumWithOid, SPI, GUC call sites compile
- [ ] `cargo clippy --no-default-features --features pg18 -- -D warnings` clean
- [ ] `cargo pgrx install` succeeds

## TDD Cycles

### Cycle 1: Version bump + compile
- **RED**: Change Cargo.toml, expect compile errors
- **GREEN**: Fix each compile error (DatumWithOid, SPI, GUC, etc.)
- **REFACTOR**: Clean up workarounds that pgrx 0.17.0 may have fixed
- **CLEANUP**: clippy clean

## Status
[ ] Not Started

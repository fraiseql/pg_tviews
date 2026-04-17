# pg_tviews Code Quality & Performance Audit — Implementation Plan

## Context

A full codebase audit identified 11 improvement areas across security, performance,
and code quality. This plan organizes them into 5 phases, ordered by priority and
dependency (earlier phases unblock later ones).

## Phase Overview

| Phase | Title | Focus | Est. Files |
|-------|-------|-------|------------|
| 1 | SQL Parameterization | Security: eliminate all string-interpolated SQL values | 3 |
| 2 | Consolidate Duplicates | Quality: merge duplicate `relname_from_oid` / `get_view_columns` | 4 |
| 3 | Audit Logging & Cache Improvements | Perf: conditional audit, negative cache, batch audit | 4 |
| 4 | Cache Backend & Dead Code | Perf/Quality: Mutex→RefCell, remove dead cascade.rs code | 3 |
| 5 | Dependency Cleanup & Finalize | Build: remove unused crates, final lint pass | 2 |

## Current Status

- [x] Audit complete
- [ ] Phase 1: SQL Parameterization
- [ ] Phase 2: Consolidate Duplicates
- [ ] Phase 3: Audit Logging & Cache Improvements
- [ ] Phase 4: Cache Backend & Dead Code
- [ ] Phase 5: Dependency Cleanup & Finalize

## Key Constraints

- pgrx 0.17.0 on PostgreSQL 18
- Clippy must stay clean: `cargo clippy --no-default-features --features pg18`
- `cargo test` (unit tests) must pass after each phase
- Do NOT break the `#[pg_test]` integration tests (they require `cargo pgrx test pg18`
  which has linker issues — verify with `cargo pgrx install` + manual testing if needed)
- Never use `catch_unwind` around SPI in transaction callbacks (see memory: SPI crash rule)
- Follow existing code style: parameterized queries use `DatumWithOid::new` + `$N` placeholders

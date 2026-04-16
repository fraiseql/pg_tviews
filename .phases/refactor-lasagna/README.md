# Lasagna Code Refactor

## Overview

Address structural duplication and unnecessary layering introduced across
the CTE / DISTINCT ON / UNION All feature work (phases 1–3). All changes
are pure refactors: no behaviour change, no new features, no SQL or API
surface changes.

Every issue was identified by diffing against `v0.1.0-beta.9`.

## Issue Summary

| ID | File | Problem | Severity |
|----|------|---------|----------|
| R1 | `schema/parser.rs` | Two 60-line functions that are identical except for their return type | HIGH |
| R2 | `catalog.rs` | Three `load_*` methods duplicate the same SPI SELECT with only the WHERE clause varying | HIGH |
| R3 | `twophase.rs` + `queue/xact.rs` | `process_refresh_queue` re-implements the propagation loop already in `flush_refresh_queue`; two dead 2PC helpers remain in xact.rs | HIGH |
| R4 | `dependency/triggers.rs` | `install_triggers` / `remove_triggers` share identical per-table preamble (name construction, quoting) | MEDIUM |
| R5 | `health.rs` + `admin.rs` | 5 hardcoded check blocks; nested match in `analyze_select` | LOW |

## Phase Order

| Phase | Covers | Risk |
|-------|--------|------|
| [1 — Parser unification](phase-01-parser.md) | R1 | Low — parser tests are comprehensive |
| [2 — Catalog loader consolidation](phase-02-catalog.md) | R2 | Low — mechanical private helper extraction |
| [3 — Queue/2PC deduplication](phase-03-queue-2pc.md) | R3 | Medium — touches transaction-critical code path |
| [4 — Trigger helper extraction](phase-04-triggers.md) | R4 | Low — pure rename/extract within one file |
| [5 — Health/admin cleanup](phase-05-health-admin.md) | R5 | Low — no logic change |
| [6 — Finalize](phase-06-finalize.md) | All | — |

## Success Criteria (overall)

- `cargo pgrx install` succeeds
- All existing tests pass (`cargo test`)
- `cargo clippy --no-default-features --features pg18 -- -D warnings` is clean
- `git grep -i "TODO\|FIXME\|phase\|hack"` returns nothing in `src/`
- Zero new public API surface introduced

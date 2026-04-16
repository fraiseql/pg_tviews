# Advanced SQL Support Phases

## Overview

Extend pg_tviews to support three SQL constructs currently rejected by the
regex-based parser: CTEs (`WITH` clauses), `DISTINCT ON`, and `UNION ALL`.

All three share a common bottleneck: `src/schema/parser.rs` assumes a flat
`SELECT ... FROM ...` structure. Dependency discovery (`dependency/graph.rs`)
and backing view creation already work for arbitrary SQL because they delegate
to PostgreSQL catalogs — so the work concentrates in **parsing** and, for
some features, **refresh scoping**.

## Phase Order (easiest → hardest)

| Phase | Issue | Feature | Risk | Key challenge |
|-------|-------|---------|------|---------------|
| 1 | #41 | CTE (`WITH`) support | Low | Parser must skip CTE preamble |
| 2 | #40 | `DISTINCT ON` support | Medium | Refresh must scope by dedup key |
| 3 | #42 | `UNION ALL` / `UNION` support | Medium-High | Parser must handle multi-branch SQL |

## Current Status

- [x] Phase 1: CTE support (#41)
- [x] Phase 2: DISTINCT ON support (#40)
- [x] Phase 3: UNION ALL / UNION support (#42)

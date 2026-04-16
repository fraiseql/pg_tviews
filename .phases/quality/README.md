# Quality Hardening Plan — pg_tviews 0.1.0

## Source

Based on two independent audits run at commit `709d517`:

- `evaluation/20260416T214944Z-security-audit.md` — 12 real findings (F-01…F-12), 4 INFO no-action
- `evaluation/20260416T215631Z-performance-audit.md` — 12 findings (P-01…P-12)

## Phase Order

| Phase | Focus | Findings | Risk |
|-------|-------|----------|------|
| 1 | Critical Safety | F-01, F-07, P-09 | HIGH — data corruption / session breakage |
| 2 | Security Hardening | F-02, F-03, F-04, F-05, F-06, F-11 | MEDIUM — SQLi, privilege, silent data loss |
| 3 | Performance — Hot Paths | P-04, P-02, P-03, P-01 | LOW risk, HIGH reward |
| 4 | Performance — Caching | P-05, P-06, P-07, P-08, P-10, P-11 | LOW risk, MEDIUM reward |
| 5 | Code Quality & Cleanup | F-08, F-09, F-10, F-12a, F-12b+P-12, F-13 | LOW |
| 6 | Finalize | archaeology, docs, release | — |

## Status

- [x] Phase 1: Critical Safety
- [x] Phase 2: Security Hardening
- [x] Phase 3: Performance — Hot Paths
- [~] Phase 4: Performance — Caching (Cycle 1 starting)
- [ ] Phase 5: Code Quality & Cleanup
- [ ] Phase 6: Finalize

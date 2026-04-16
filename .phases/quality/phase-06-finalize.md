# Phase 6: Finalize

## Objective

Transform the hardened codebase into a clean, production-ready 0.1.0 release.
No phase markers, no dead code, no development archaeology, no open TODO/FIXME.

## Success Criteria

- [ ] `git grep -iE "phase|todo|fixme|hack|workaround|dead.code"` returns nothing except
  legitimate uses (e.g., the pgrx workaround comment updated in Phase 5)
- [ ] `cargo clippy --no-default-features --features pg18 -- -D warnings` clean
- [ ] `cargo test` passes
- [ ] Release build succeeds: `cargo pgrx package`
- [ ] README accurately reflects all GUCs (including `max_queue_size`), extension
  requirements, and known limitations (2PC not supported)
- [ ] No `#[allow(...)]` attributes without a `// Reason:` comment justifying them

## Steps

### 1. Quality Control Review

Review the full src/ tree as a senior engineer would:

- [ ] API surface (`pg_tviews_create`, `pg_tviews_refresh`, `pg_tviews_drop`,
  `pg_tviews_convert_table`) is consistent in naming and error messages
- [ ] All `error!()` messages include enough context for a user to self-diagnose
- [ ] No unnecessary allocations or clones in hot paths remain
- [ ] Caches have a coherent invalidation story (all cleared by `invalidate_all_caches`)
- [ ] SAVEPOINT semantics are correct end-to-end (Phase 1 fixes held through all changes)

### 2. Security Audit Re-check

Verify every MEDIUM/HIGH finding from `evaluation/20260416T214944Z-security-audit.md`
is closed:

- [ ] F-01: HOOK_IN_PROGRESS cannot be leaked (integration test proves it)
- [ ] F-02: `entity_name` validated before use; `register_metadata` fully parameterized
- [ ] F-03: multi-table DROP TABLE processes all names
- [ ] F-04: audit log records `session_user`
- [ ] F-05: `PREPARE TRANSACTION` with pending queue raises error
- [ ] F-06: no manual SQL string escaping in convert.rs

### 3. Archaeology Removal

- [ ] Remove all `// Phase X` and `// Cycle X` comments
- [ ] Remove all `// TODO` and `// FIXME` (fix or delete)
- [ ] Remove all commented-out code
- [ ] Remove all `#[allow(dead_code)]` annotations (fixed in Phase 5)
- [ ] Verify `evaluation/` directory stays (it's audit history, not development debris)
- [ ] Verify `.phases/` is excluded from shipped extension artifacts

### 4. Documentation Polish

- [ ] README sections:
  - Features (update with DISTINCT ON, CTE, UNION ALL support from previous phases)
  - Configuration (add `pg_tviews.max_queue_size` and `pg_tviews.max_propagation_depth`)
  - Limitations (2PC not supported in 0.1.0)
  - Performance notes (caching, when to expect bulk-DML overhead)
- [ ] All `///` doc comments on public functions are accurate
- [ ] No references to internal implementation phases in user-facing text

### 5. Final Verification

```bash
# Zero warnings
cargo clippy --no-default-features --features pg18 -- -D warnings

# No archaeology
git grep -iE 'phase [0-9]|todo[:(]?\b|fixme[:(]?\b|hack[:(]?\b|workaround'

# No unexplained allows
git grep '#\[allow(' src/

# All tests green
cargo test --no-default-features --features pg18

# Release build
cargo pgrx package --features pg18
```

### 6. Release Commit

After all checks pass:

```
chore(release): finalize 0.1.0

All audit findings resolved:
- F-01–F-12: security hardening complete
- P-01–P-11: performance optimizations applied
- Zero dead code, zero clippy warnings

See evaluation/ for full audit reports.
```

## Dependencies

- Requires: Phases 1–5 complete
- Blocks: Nothing (this is the final phase)

## Status

[ ] Not Started

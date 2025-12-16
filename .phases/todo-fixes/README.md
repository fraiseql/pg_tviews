# TODO Fixes Phase Plans

This directory contains implementation plans for addressing all TODO items identified in the pg_tviews codebase.

## Phase Overview

| Phase | Title | Priority | Complexity | Status |
|-------|-------|----------|------------|--------|
| 1 | [Savepoint Depth Tracking](phase-1-savepoint-depth.md) | **High** | Low | Pending |
| 2 | [GUC Configuration System](phase-2-guc-configuration.md) | Medium | Medium | Pending |
| 3 | [Queue Introspection](phase-3-queue-introspection.md) | Medium | Low | Pending |
| 4 | [Dynamic PK Column Detection](phase-4-dynamic-pk-detection.md) | **High** | Medium | Pending |
| 5 | [Cache Refresh Integration](phase-5-cache-refresh-integration.md) | Low | Medium | Pending |
| 6 | [TEXT[][] Workaround](phase-6-text-array-workaround.md) | Medium | Medium | Pending |
| 7 | [Error Mapping](phase-7-error-mapping.md) | Low | Low | Pending |

## Priority Legend

- **High**: Affects correctness or core functionality
- **Medium**: Improves usability or enables features
- **Low**: Polish, optimization, or future-proofing

## Recommended Implementation Order

### Immediate (Correctness Issues)

1. **Phase 1: Savepoint Depth** - Required for correct savepoint/rollback behavior
2. **Phase 4: Dynamic PK Detection** - Required for multi-entity support

### Short-term (Feature Enablement)

3. **Phase 6: TEXT[][] Workaround** - Enables nested JSONB refresh
4. **Phase 3: Queue Introspection** - Enables monitoring

### Medium-term (Usability)

5. **Phase 2: GUC Configuration** - Runtime configuration
6. **Phase 7: Error Mapping** - Better error messages

### Long-term (Optimization)

7. **Phase 5: Cache Refresh Integration** - Performance optimization

## TODO Source Locations

| File | Line | TODO Description | Phase |
|------|------|------------------|-------|
| `src/queue/persistence.rs` | 144-148 | `get_savepoint_depth()` stub | 1 |
| `src/config/mod.rs` | 30,36,42,48,54 | GUC configuration | 2 |
| `src/metadata.rs` | 123 | Queue introspection | 3 |
| `src/utils.rs` | 40 | Dynamic column detection | 4 |
| `src/refresh/cache.rs` | 70 | Cache refresh integration | 5 |
| `src/catalog.rs` | 115, 182 | TEXT[][] extraction | 6 |
| `src/lib.rs` | 712 | TEXT[][] extraction | 6 |
| `src/error/mod.rs` | 414 | Error mapping | 7 |
| `src/dependency/graph.rs` | 228 | Parser API (v2) | Deferred |

## Deferred Items

Some TODOs are intentionally deferred to future major versions:

- **PostgreSQL Parser API** (`src/dependency/graph.rs:228`): Requires significant refactoring, planned for v2.0
- **Composite Primary Keys**: Would require schema changes to `pg_tview_meta`
- **UUID Primary Keys**: Currently only i64 PKs supported

## Verification

After implementing any phase, run:

```bash
# Build check
cargo check --no-default-features --features pg18

# Clippy
cargo clippy --no-default-features --features pg18 -- -D warnings

# Tests
cargo pgrx test pg18
```

## Contributing

When implementing a phase:

1. Read the phase plan thoroughly
2. Follow the implementation steps in order
3. Run verification commands after each step
4. Check acceptance criteria before marking complete
5. Update this README with status

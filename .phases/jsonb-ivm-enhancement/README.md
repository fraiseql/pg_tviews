# jsonb_ivm Enhancement Implementation

**Project**: Integrate advanced jsonb_ivm functions into pg_tviews
**Status**: Ready for implementation
**Target Version**: pg_tviews 0.2.0

---

## Quick Start for Junior Developers

1. **Read this file first** to understand the project structure
2. **Read `00-IMPLEMENTATION-PLAN.md`** for overall architecture and strategy
3. **Start with Phase 1** (`phase-1-helper-functions.md`) - easiest entry point
4. **Work sequentially** through Phases 2-5
5. **Run tests after each phase** before proceeding to the next

---

## Project Overview

This implementation plan adds 4 high-value functions from the `jsonb_ivm` extension to pg_tviews:

| Function | Benefit | Phase |
|----------|---------|-------|
| `jsonb_array_contains_id()` | 10× faster existence checks | 1 |
| `jsonb_extract_id()` | 5× faster ID extraction | 1 |
| `jsonb_ivm_array_update_where_path()` | 2-3× faster nested updates | 2 |
| `jsonb_array_update_where_batch()` | 3-5× faster bulk operations | 3 |
| `jsonb_ivm_set_path()` | 2× faster fallback updates | 4 |

**Total Expected Performance Gain**: 2-10× across different operations

---

## Directory Structure

```
.phases/jsonb-ivm-enhancement/
├── README.md                          ← You are here
├── 00-IMPLEMENTATION-PLAN.md          ← Start here: Overall plan
├── phase-1-helper-functions.md        ← Phase 1: Easy (1-2 hours)
├── phase-2-nested-path-updates.md     ← Phase 2: Medium (2-3 hours)
├── phase-3-batch-operations.md        ← Phase 3: Medium-High (3-4 hours)
├── phase-4-fallback-paths.md          ← Phase 4: Easy (1-2 hours)
└── phase-5-integration-testing.md     ← Phase 5: Testing (2-3 hours)
```

**Total Time**: ~10-15 hours for complete implementation

---

## Phase Summary

### Phase 1: Helper Functions ⭐ START HERE
**Difficulty**: 🟢 LOW
**Time**: 1-2 hours
**Files**: `src/utils.rs`, `src/refresh/array_ops.rs`

Add simple wrapper functions for:
- Fast ID extraction from JSONB
- Fast array element existence checking
- Safe array insertion (prevents duplicates)

**Why start here**: Low risk, high value, easy to understand

---

### Phase 2: Nested Path Updates
**Difficulty**: 🟡 MEDIUM
**Time**: 2-3 hours
**Files**: `src/catalog.rs`, `src/refresh/array_ops.rs`, `src/refresh/main.rs`

Enable updating nested fields within array elements:
- Extend metadata to track nested paths
- Add path-based array update function
- Integrate with cascade logic

**Example**: Update `comment.author.name` without rebuilding entire comment

---

### Phase 3: Batch Operations
**Difficulty**: 🟡 MEDIUM-HIGH
**Time**: 3-4 hours
**Files**: `src/refresh/bulk.rs`, `src/refresh/batch.rs`

Enable bulk updates to multiple array elements:
- Add batch update function
- Optimize batch sizes
- Integrate with bulk refresh engine

**Example**: Update prices for 50 products in one operation vs 50 sequential updates

---

### Phase 4: Fallback Path Operations
**Difficulty**: 🟢 LOW
**Time**: 1-2 hours
**Files**: `src/refresh/main.rs`

Add flexible path-based updates as fallback:
- Integrate `jsonb_ivm_set_path()` into apply_patch()
- Handle unknown/complex structures
- Graceful degradation chain

**Example**: Update any nested path when metadata incomplete

---

### Phase 5: Integration Testing ⚠️ CRITICAL
**Difficulty**: 🟡 MEDIUM
**Time**: 2-3 hours
**Files**: `test/sql/*`, `docs/*`

Comprehensive testing and validation:
- End-to-end integration tests
- Performance benchmarks
- Regression tests
- Documentation updates

**Why critical**: Validates everything works together and meets performance targets

---

## How to Use This Plan

### For Individual Developers

1. **Clone the repository**
   ```bash
   cd pg_tviews
   ```

2. **Read the implementation plan**
   ```bash
   cat .phases/jsonb-ivm-enhancement/00-IMPLEMENTATION-PLAN.md
   ```

3. **Start with Phase 1**
   ```bash
   cat .phases/jsonb-ivm-enhancement/phase-1-helper-functions.md
   ```

4. **Follow the steps exactly**
   - Read entire phase file before starting
   - Implement step by step
   - Run verification after each step
   - Only proceed to next phase after verification passes

5. **Commit after each phase**
   ```bash
   git add .
   git commit -m "feat(helpers): Add jsonb_extract_id and jsonb_array_contains_id wrappers [PHASE1]"
   ```

### For Teams

**Parallel Execution**:
- Developer A: Phase 1 → Phase 2 → Phase 5
- Developer B: Phase 1 → Phase 3 → Phase 5

**Sequential Execution**:
- Week 1: Phases 1-2
- Week 2: Phases 3-4
- Week 3: Phase 5 + Documentation

---

## Prerequisites

### Required Knowledge

- Basic Rust programming
- PostgreSQL SQL syntax
- JSONB data type understanding
- Git version control

### System Requirements

- Rust 1.70+ installed
- PostgreSQL 13+ installed
- pgrx 0.12+ installed
- jsonb_ivm extension installed (from `../jsonb_ivm`)

### Setup

```bash
# Install pgrx
cargo install --locked cargo-pgrx
cargo pgrx init

# Install jsonb_ivm (prerequisite)
cd ../jsonb_ivm
cargo pgrx install --release

# Return to pg_tviews
cd ../pg_tviews
```

---

## Testing Strategy

Each phase includes three levels of testing:

1. **Unit Tests** (Rust)
   ```bash
   cargo test
   ```

2. **Integration Tests** (SQL)
   ```bash
   psql -d test_db -f test/sql/XX-test-name.sql
   ```

3. **Verification Steps**
   - Specific commands in each phase file
   - Expected output documented
   - Acceptance criteria checklist

---

## Getting Help

### If You Get Stuck

1. **Check the troubleshooting section** in the phase file
2. **Re-read the context section** at the top of the phase
3. **Review the DO NOT section** - you might be violating a constraint
4. **Check existing tests** for examples
5. **Ask for help** with specific error messages

### Common Issues

**Build Errors**:
- Ensure jsonb_ivm is installed first
- Check Rust version: `rustc --version`
- Clean build: `cargo clean && cargo pgrx install --release`

**Test Failures**:
- Verify database has both extensions installed
- Check for typos in SQL commands
- Review error messages carefully

**Performance Not Improving**:
- Verify jsonb_ivm actually being used (check function existence)
- Review SQL EXPLAIN ANALYZE output
- Check data sizes (small datasets may not show improvement)

---

## Success Criteria

### Per-Phase Criteria

Each phase file has specific acceptance criteria. All must pass before proceeding.

### Overall Success Criteria

- ✅ All 5 phases implemented
- ✅ All unit tests pass (`cargo test`)
- ✅ All integration tests pass
- ✅ Performance improvements validated
- ✅ No clippy warnings (`cargo clippy`)
- ✅ Documentation complete
- ✅ Backward compatibility maintained

---

## Timeline

### Conservative Estimate
- Phase 1: 2 hours
- Phase 2: 3 hours
- Phase 3: 4 hours
- Phase 4: 2 hours
- Phase 5: 3 hours
- **Total**: 14 hours (~2 working days)

### Optimistic Estimate
- Phase 1: 1 hour
- Phase 2: 2 hours
- Phase 3: 3 hours
- Phase 4: 1 hour
- Phase 5: 2 hours
- **Total**: 9 hours (~1.5 working days)

**Recommendation**: Plan for 2-3 days including testing and documentation.

---

## Code Quality Standards

All code must meet these standards:

1. **Rust Code**
   - Pass `cargo clippy --all-targets --all-features -- -D warnings`
   - Pass `cargo fmt --check`
   - No `unwrap()` in production code (use `?` operator)
   - Comprehensive error handling
   - Documentation comments on all public functions

2. **SQL Code**
   - Parameterized queries (no SQL injection)
   - Proper quoting of identifiers
   - Error handling in DO blocks

3. **Tests**
   - Cover happy path
   - Cover error cases
   - Cover edge cases
   - Clear test names and messages

---

## Documentation Requirements

Each phase must update:

1. **Code Documentation**
   - Rustdoc comments on functions
   - SQL comments in test files
   - Inline comments for complex logic

2. **User Documentation** (Phase 5)
   - API reference updates
   - Performance guide updates
   - Migration guide

3. **Git Commits**
   - Descriptive commit messages
   - Reference phase in commit

---

## After Completion

Once all phases complete:

1. **Tag the release**
   ```bash
   git tag v0.2.0-jsonb-ivm-enhanced
   git push --tags
   ```

2. **Update CHANGELOG.md**
   - List all new features
   - Document performance improvements
   - Note any breaking changes (should be none)

3. **Create GitHub release**
   - Attach binaries if applicable
   - Link to documentation
   - Highlight key improvements

4. **Announce**
   - Update README.md with performance numbers
   - Blog post (optional)
   - Social media (optional)

---

## Questions?

If anything in this plan is unclear:

1. Check the `00-IMPLEMENTATION-PLAN.md` for architecture context
2. Review the specific phase file for detailed steps
3. Look at existing similar code in the codebase
4. Ask maintainers for clarification

---

## Let's Begin! 🚀

**Start here**: Open `phase-1-helper-functions.md` and follow the steps.

**Remember**:
- Read the entire phase before coding
- Test after every change
- Commit after every phase
- Ask for help when stuck
- Have fun! 😊

Good luck! You've got this! 💪

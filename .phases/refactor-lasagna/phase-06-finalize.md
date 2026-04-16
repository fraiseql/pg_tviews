# Phase 6: Finalize

## Objective

Verify the completed refactor is clean, leaves no development archaeology,
and is ready for release tagging.

## Steps

### 1. Full verification pass

```bash
# Build
cargo pgrx install --no-default-features --features pg18

# Tests
cargo test --no-default-features --features pg18

# Lints (zero warnings allowed)
cargo clippy --no-default-features --features pg18 -- -D warnings

# No phase markers, TODOs, or FIXMEs in src/
git grep -i "TODO\|FIXME\|phase\|hack" -- src/
```

All four commands must succeed cleanly.

### 2. Code archaeology check

Confirm nothing was left behind from the refactor:

- [ ] No `#[allow(dead_code)]` without a `// Reason:` comment in any changed file
- [ ] No commented-out code blocks
- [ ] No `process_refresh_queue` in `twophase.rs`
- [ ] No `handle_prepare` or `get_prepared_transaction_id` in `queue/xact.rs`
- [ ] No `extract_columns_with_expressions_regex` as a standalone function in `parser.rs`

```bash
git grep "process_refresh_queue\|handle_prepare\|get_prepared_transaction_id\|extract_columns_with_expressions_regex" -- src/
# Expected: zero results (refresh_and_get_parents should only exist in queue/xact.rs, not twophase.rs)
git grep "refresh_and_get_parents" -- src/twophase.rs
# Expected: zero results
```

### 3. API surface check

Confirm no new public symbols were accidentally exposed:

```bash
git diff v0.1.0-beta.9..HEAD -- src/lib.rs | grep "^+pub"
# Review any additions — they should be zero for this refactor
```

### 4. Diff review

```bash
git diff v0.1.0-beta.9..HEAD -- src/schema/parser.rs src/catalog.rs \
    src/queue/xact.rs src/twophase.rs src/dependency/triggers.rs \
    src/health.rs src/admin.rs
```

For each file, confirm:
- The total line count went down or stayed flat
- No new `unsafe` blocks were introduced
- No new `unwrap()` without justification

### 5. Remove `.phases/refactor-lasagna/`

Once all checks pass and the release tag is applied:

```bash
git rm -r .phases/refactor-lasagna/
git commit -m "chore: remove refactor-lasagna phase plan after completion"
```

## Success Criteria

- [ ] `cargo pgrx install` succeeds
- [ ] `cargo test` passes
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `git grep -i "TODO\|FIXME"` returns nothing in `src/`
- [ ] No removed functions appear in `src/` (verified by grep)
- [ ] `.phases/refactor-lasagna/` deleted from main branch
- [ ] CHANGELOG.md has an entry under `[Unreleased]` for the refactor

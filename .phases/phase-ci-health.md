# Phase: CI/CD Health & Project Hygiene

## Objective

Bring all GitHub Actions badges to green on `main`, fix version inconsistencies in the README, and clean up stale CI workflows.

## Current State Assessment (2026-03-19)

### Badge Status on `main`

| Badge | Workflow File | Status | Root Cause |
|-------|--------------|--------|------------|
| CI | `ci.yml` | **FAILING** | cargo-pgrx 0.17.0 installed, project uses pgrx 0.16.1 |
| Clippy Strict | `clippy.yml` | **FAILING** | 30+ `doc_markdown` lint errors (missing backticks) |
| Code Coverage | `coverage.yml` | **FAILING** | Same pgrx version mismatch as CI |
| Security Audit | `security-audit.yml` | **PASSING** | Only daily cron runs; works fine |
| Documentation | `docs.yml` | **PASSING** | Works |
| Performance | `performance.yml` | **FAILING** | pgrx mismatch + references dead branch `benchmark-fixes-20251214` |

### README Badge Issues

- Badges point to `?branch=dev`, but most recent work is on `main`
- "Documentation" badge references `documentation.yml` but the file is `docs.yml`
- Version badge says `0.1.0-beta.3` but Cargo.toml is at `0.1.0-beta.9`
- "PostgreSQL 13-18" badge but CI only tests PG16
- "Rust 1.70+" badge — likely outdated (pgrx 0.16.1 requires newer)
- "Current Version: 0.1.0-beta.1 (December 2025)" in body text

### Workflow Issues

| Workflow | Issue |
|----------|-------|
| `ci.yml` | Installs `cargo-pgrx` from crates.io (gets latest 0.17.0), needs pinned version |
| `clippy.yml` | Pedantic `doc_markdown` violations in `src/lib.rs` (30+ errors) |
| `coverage.yml` | Same pgrx pin issue as ci.yml; not real coverage, just build verification |
| `performance.yml` | References non-existent branch; uses PG17 while CI uses PG16 |
| `docker.yml` | References non-existent `Dockerfile.benchmarks` |
| `slsa-provenance.yml` | startup_failure on last tag push |
| `release.yml` | Failed on v0.1.0-beta.9 tag |

### Stale Dependabot Branches

- `dependabot/github_actions/actions/github-script-8`
- `dependabot/github_actions/actions/upload-artifact-6`
- `dependabot/github_actions/docker/build-push-action-6`
- `dependabot/github_actions/github/codeql-action-4`
- `dependabot/github_actions/slsa-framework/...`

### Stale Dev Branch

`dev` branch is 3 months behind `main` (last green: 2026-01-01). Last push (2026-02-22) failed. All active development is on `main`.

---

## TDD Cycles

### Cycle 1: Pin cargo-pgrx version in all workflows

**RED**: Push a branch, observe CI fails with pgrx version mismatch.
**GREEN**: In every workflow that runs `cargo install cargo-pgrx`, pin to `--version 0.16.1 --locked`. Affected files:
  - `ci.yml`
  - `coverage.yml`
  - `performance.yml`
  - `release.yml`
  - `slsa-provenance.yml`

**REFACTOR**: Extract pgrx version into a workflow-level `env:` variable or use a reusable workflow to avoid version drift across files.
**CLEANUP**: Verify CI passes on branch before merging.

### Cycle 2: Fix Clippy pedantic doc_markdown violations

**RED**: `cargo clippy --no-default-features --features pg16 -- -D warnings` fails with ~30 `doc_markdown` errors.
**GREEN**: Add backticks around function names, column names, and identifiers in doc comments throughout `src/lib.rs` (and any other affected files).
**REFACTOR**: Review doc comments for accuracy while fixing formatting.
**CLEANUP**: Verify `clippy.yml` workflow passes.

### Cycle 3: Fix README badges and version references

**GREEN**: Update README.md:
  1. Change all badge URLs from `?branch=dev` to `?branch=main`
  2. Fix "Documentation" badge: `documentation.yml` → `docs.yml`
  3. Update version badge from `0.1.0-beta.3` to `0.1.0-beta.9`
  4. Update "Current Version" text in body
  5. Update Rust minimum version badge to match actual MSRV
  6. Update PostgreSQL version range to match what CI actually tests

**CLEANUP**: Verify all badge URLs resolve and show green.

### Cycle 4: Fix or remove broken workflows

**GREEN**:
  - `docker.yml`: Either create `Dockerfile.benchmarks` or remove/disable this workflow (no Docker image is being published)
  - `performance.yml`: Remove dead `benchmark-fixes-20251214` branch from triggers; align PG version with CI
  - `coverage.yml`: Decide: make it real code coverage (with tarpaulin/llvm-cov) or rename to "Integration Test" to avoid misleading badge
  - `slsa-provenance.yml`: Debug startup_failure — likely needs permissions or workflow syntax fix

**REFACTOR**: Consider consolidating overlapping workflows (CI + Coverage are nearly identical).
**CLEANUP**: Remove or archive workflows that cannot realistically work for a pgrx extension (true code coverage is very hard with pgrx).

### Cycle 5: Clean up stale branches

**GREEN**:
  - Merge or close stale Dependabot PRs
  - Decide fate of `dev` branch: delete if all work is on `main`, or sync it
  - Delete `pre-commit-ci-update-config` if stale

**CLEANUP**: Verify no orphaned branch references remain in workflow triggers.

### Cycle 6: Verify end-to-end

- Push to `main` (or PR to `main`)
- Confirm ALL referenced badges show green
- Confirm release workflow would succeed on next tag

---

## Dependencies

- Requires: Access to GitHub repo settings (for branch cleanup)
- Blocks: Any future release tagging (release.yml must work first)

## Priority Order

1. **Cycle 1** (pgrx pin) — unblocks CI, Coverage, Performance, Release
2. **Cycle 2** (clippy fixes) — unblocks Clippy badge
3. **Cycle 3** (README) — cosmetic but high visibility
4. **Cycle 4** (broken workflows) — clean up noise
5. **Cycle 5** (branches) — housekeeping
6. **Cycle 6** (verification) — final check

## Estimated Scope

- Cycle 1: ~11 workflow files to audit, 5 need pgrx pin fix
- Cycle 2: ~30 doc comment fixes in src/lib.rs, possibly other files
- Cycle 3: ~10 line changes in README.md
- Cycle 4: 3-4 workflow files to fix or remove
- Cycle 5: 5-6 branches to clean up
- Cycle 6: Push and verify

## Status
[x] Cycle 1: Pin cargo-pgrx 0.16.1 in all workflows (ci, clippy, coverage, performance, release, docs, sbom)
[x] Cycle 2: Fix all 40 clippy pedantic violations (21 doc_markdown, 10 redundant closures, 3 let-else, 2 cast_possible_wrap, 1 cast_sign_loss, 1 match_same_arms, 1 implicit_borrow, 1 if_let)
[x] Cycle 3: Fix README badges (branch=main, correct workflow filenames, version 0.1.0-beta.9, Rust 1.81+, PG16)
[x] Cycle 4: Fix broken workflows (docker disabled, perf aligned to PG16, coverage renamed, slsa job ordering, deprecated actions updated)
[x] Cycle 5: Clean up stale branches (closed 6 PRs, deleted 6 remote branches, only main remains)
[x] Cycle 6: Verify end-to-end — all 6 workflows green on main (CI, Clippy, Integration, Docs, Perf, Security)

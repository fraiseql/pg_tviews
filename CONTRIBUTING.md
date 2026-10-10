# Contributing to pg_tviews

## Workflow

- Every change starts with a test that fails for the right reason: a regress file
  in `test/sql/regress/<feature>/`, an isolation spec, or a unit test
  ([docs/development/testing.md](docs/development/testing.md)). Prove it fails on
  the code without your change, then make it pass.
- A change to the extension SQL updates the pending upgrade script and keeps a fresh
  install and an upgraded one identical
  ([docs/development/extension-versioning.md](docs/development/extension-versioning.md)).
- User-visible behaviour goes under `## [Unreleased]` in `CHANGELOG.md`.
- A change that touches a write path is benchmarked against the base commit on one
  machine before it is merged (`test/sql/real_benchmark/README.md`).

## Lint policy

`Cargo.toml` holds the policy CI enforces (`cargo clippy --all-targets -- -D warnings`
on PostgreSQL 16, 17 and 18):

- clippy `all`, `pedantic` and `cargo` deny, `nursery` warns (and so fails under
  `-D warnings`); `undocumented_unsafe_blocks` and `unsafe_op_in_unsafe_fn` deny;
  `missing_docs` warns.
- The crate-wide exceptions are listed there, each with its reason. Anything
  narrower is allowed on the item, with a `// Reason:` comment.
- `cargo fmt --check` and `scripts/check-ffi-guards.sh` (every callback PostgreSQL
  calls carries `#[pg_guard]`) run in CI too.

### `unsafe` code

The global rule for Rust projects here is `unsafe_code = "forbid"`. pg_tviews is
waived from it: a PostgreSQL extension reads executor, trigger and catalog
structures through pgrx's FFI bindings, and installs hooks PostgreSQL calls by
function pointer, which no safe API covers. In exchange:

- `unsafe` is used only for FFI with PostgreSQL (pointers it hands over, its
  functions and globals);
- every `unsafe` block carries a `// SAFETY:` comment saying why its preconditions
  hold, and every `unsafe fn` documents them (`undocumented_unsafe_blocks` and
  `unsafe_op_in_unsafe_fn` deny);
- code that needs no PostgreSQL is safe Rust, unit-tested without a server.

## Pull requests

CI runs every suite on every pull request, stacked ones included. Keep commits
focused, with a message that says what changed and why.

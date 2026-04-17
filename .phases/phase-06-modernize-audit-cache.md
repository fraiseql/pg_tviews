# Phase 6: Transaction-Scoped Audit Batching

## Objective

Replace per-statement SPI audit inserts with a transaction-scoped buffer that flushes once via bulk INSERT at commit time, and make audit logging opt-in by default.

## Why This Redesign

The current audit implementation has two limitations:
- Audit logging is opt-out (default: true), adding overhead for users who never asked for it
- Each `log_create`/`log_drop`/`log_refresh` call does an individual SPI INSERT, which is wasteful when a single transaction touches many entities

The fix is straightforward: buffer entries in a thread-local `Vec`, flush them as a single bulk INSERT in the existing ProcessUtility hook COMMIT interception path (where SPI is still safe), and flip the default to opt-in.

## Success Criteria

- [x] Audit GUC default changed to `false` (opt-in)
- [x] Audit entries buffered in thread-local `Vec<AuditEntry>` during transaction
- [x] Single bulk INSERT via `flush_audit_buffer()` called from ProcessUtility COMMIT hook
- [x] Buffer cleared on ABORT via xact callback (no SPI needed for clear)
- [x] `transaction_id` and `rows_affected` added as top-level columns to `pg_tview_audit_log`
- [x] Single index on `(entity, performed_at)` — drop redundant indexes
- [x] Existing convenience wrappers (`log_create`, `log_drop`, `log_refresh`) preserved
- [x] `cargo clippy --no-default-features --features pg18` clean
- [x] Manual testing passes (CREATE, DROP, REFRESH audit entries verified)

## Constraints

- **SPI is NOT available in transaction callbacks** (`xact.rs`). The audit flush MUST happen in the ProcessUtility hook's COMMIT interception, alongside the existing `flush_refresh_queue()` call — never in an xact callback.
- Buffer clear on ABORT is safe in xact callback (no SPI, just `Vec::clear()`).
- Keep the existing JSONB `details` column for flexible metadata — do not normalize operation-specific fields into separate columns.

## Implementation Plan

### `src/audit.rs` — Rewrite to buffer + flush

Replace direct SPI inserts with:
1. `AuditEntry` struct holding entity, operation, rows_affected, details, timestamp
2. Thread-local `AUDIT_BUFFER: RefCell<Vec<AuditEntry>>`
3. `log_operation()` pushes to buffer (no SPI, no GUC check — check at flush time)
4. `flush_audit_buffer()` does a single bulk INSERT with UNNEST, gated by `audit_enabled()` GUC
5. `clear_audit_buffer()` for ABORT path (just empties the Vec)

### `src/config/mod.rs` — Flip default

Change `AUDIT_ENABLED_GUC` default from `true` to `false`.

### `src/hooks.rs` — Call flush in COMMIT path

Add `audit::flush_audit_buffer()` call right after `flush_refresh_queue()` in the ProcessUtility hook's COMMIT interception block.

### `src/queue/xact.rs` — Clear buffer on ABORT

Add `audit::clear_audit_buffer()` call in the ABORT branch of the xact callback (safe — no SPI, just Vec::clear).

### Schema — Add columns, consolidate indexes

Add `transaction_id BIGINT` and `rows_affected BIGINT` as top-level columns to `pg_tview_audit_log`. Keep `details JSONB` for extended metadata. Replace the current indexes with a single `(entity, timestamp)` index.

## TDD Cycles

### Cycle 1: Audit Buffer and Flush

- **RED**: Test that `log_create`/`log_drop`/`log_refresh` buffer entries without writing to DB, and that `flush_audit_buffer()` writes all buffered entries in one go
- **GREEN**: Implement `AuditEntry`, thread-local buffer, `log_operation()` convenience wrappers, and `flush_audit_buffer()` with bulk INSERT
- **REFACTOR**: Remove old per-statement SPI insert logic from audit functions; wire flush into ProcessUtility COMMIT path and clear into xact ABORT callback
- **CLEANUP**: `cargo clippy --no-default-features --features pg18`, `cargo test`

### Cycle 2: Schema Update and GUC Default

- **RED**: Test that audit table has `transaction_id` and `rows_affected` columns; test that audit is disabled by default and no rows are written without explicit `SET pg_tviews.audit_enabled = true`
- **GREEN**: Update schema DDL to add new columns and consolidate indexes; flip GUC default to `false`; gate `flush_audit_buffer()` on GUC check
- **REFACTOR**: Update any queries that read from audit table to use new column layout
- **CLEANUP**: `cargo clippy --no-default-features --features pg18`, `cargo test`

## Migration Strategy

Breaking change:
1. `pg_tview_audit_log` schema changes (new columns, fewer indexes)
2. Audit disabled by default — users must `SET pg_tviews.audit_enabled = true` to opt in

## Dependencies

- Requires: Phase 1-5 complete
- Blocks: Phase 7 (finalization)

## Status
[x] Complete

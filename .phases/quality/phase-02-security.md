# Phase 2: Security Hardening

## Objective

Address all MEDIUM-severity security and correctness findings:
SQL injection via unvalidated entity_name; multi-table DROP TABLE silently orphaning
metadata; spoofable audit attribution; silent queue loss on PREPARE TRANSACTION;
incorrect JSONB escaping; unquoted identifiers in pg_tviews_refresh.

## Success Criteria

- [ ] `validate_sql_identifier(entity_name)` called before any use of `entity_name` in DDL
- [ ] `register_metadata` uses parameterized SPI queries (no format-string SQL)
- [ ] `DROP TABLE tv_a, tv_b` correctly processes all tv_* names and invokes standard
  handler for any non-tv_* tables in the same statement
- [ ] Audit log records `session_user`, not `current_user`
- [ ] `PREPARE TRANSACTION` on a backend with pending TVIEW refreshes raises a clear error
  instead of silently discarding the queue
- [ ] `reconstruct_as_tview` uses parameterized SPI INSERT instead of manual escaping
- [ ] `pg_tviews_refresh` quotes all relation and column names via `quote_identifier`
- [ ] `cargo clippy --no-default-features --features pg18 -- -D warnings` clean
- [ ] All existing tests pass

## TDD Cycles

### Cycle 1: F-02 — SQL injection in OID lookups and populate (ddl/create.rs:369,452-468, ddl/mod.rs:70)

**ROOT CAUSE:**
`pg_tviews_create(tview_name, select_sql)` validates `tview_name` via
`validate_sql_identifier`, but the effective `entity_name` comes from
`infer_schema(select_sql)` and is never validated.

Note: `register_metadata` (line 516) already uses parameterized queries (`$1`) — it is
NOT the injection surface. The actual vulnerable call sites are:

1. **OID-lookup queries** (lines 452–468): embed `view_name`, `schema_name`, and
   `tview_name` via format strings:
   `format!("SELECT c.oid ... WHERE c.relname = '{view_name}' AND n.nspname = '{schema_name}'")`
2. **`populate_initial_data`** (line 369): builds
   `INSERT INTO {schema_name}.{tview_name}` with unquoted/unparameterized names.

A crafted SELECT with a quoted column identifier containing SQL metacharacters could
return a wrong OID, corrupting `pg_tview_meta`.

- **RED**: Write a test that calls `pg_tviews_create` with a SELECT whose column alias
  contains a single-quote. Assert it fails with a validation error, not silently succeeds.
- **GREEN**:
  1. Add `validate_sql_identifier(&entity_name, "entity_name")?` in `create_tview`
     (ddl/mod.rs) immediately after line 70, before any use of `entity_name`.
  2. Replace the two unparameterized OID-lookup queries (ddl/create.rs:452-468) with
     `Spi::get_one_with_args`, passing `view_name` and `schema_name` as `$1`/`$2` text
     parameters.
  3. In `populate_initial_data` (line 369), quote `schema_name` and `tview_name` with
     `quote_identifier` or use parameterized queries where possible.
- **REFACTOR**: Extract parameterized-query builder if used in more than one place.
- **CLEANUP**: Confirm no remaining `format!("... '{view_name}' ...")` or unquoted
  `format!("... {tview_name} ...")` patterns in ddl/create.rs.

### Cycle 2: F-03 — multi-table DROP TABLE processes only first tv_* name (hooks.rs:407-445)

**ROOT CAUSE:**
`handle_drop_table` splits the raw query string on whitespace and stops at the *first*
word starting with `tv_`. For `DROP TABLE tv_foo, tv_bar`, only `tv_foo` is processed.
The hook returns `true` (handled), suppressing the standard handler for the whole
statement. `tv_bar` metadata and triggers are orphaned. For `DROP TABLE regular, tv_foo`,
`regular` is never dropped.

- **RED**: Write a pgtest: `DROP TABLE tv_a, tv_b`. Assert both are fully removed from
  `pg_tview_meta` and their triggers are gone.
  Second test: `DROP TABLE regular_table, tv_foo`. Assert both are dropped from their
  respective catalogs.
- **GREEN**: Replace the raw string-splitting approach with iteration over the parsed
  `DropStmt.objects` list (available via the `drop_stmt: *mut pg_sys::DropStmt` pointer
  already passed to the function):
  1. Collect all relation names from `drop_stmt.objects`.
  2. Partition into `tv_*` names and non-`tv_*` names.
  3. Call `drop_tview` for each tv_* name.
  4. If non-tv_* names exist, call `call_prev_hook_or_standard` for those (or for the
     whole statement after overriding the objects list to exclude the tv_* ones).
- **REFACTOR**: Ensure each `drop_tview` call is independent (failure of one should not
  block others — consider collecting errors and emitting after all drops).
- **CLEANUP**: Delete the old whitespace-split code; clippy clean.

### Cycle 3: F-04 — audit log uses current_user instead of session_user (audit.rs:6-7,28-29,47-48)

**ROOT CAUSE:**
`log_create`, `log_drop`, and `log_refresh` record `performed_by` via
`SELECT current_user::text`. `current_user` reflects the active role after `SET ROLE`,
not the original authenticated identity. This makes the audit trail spoofable.

- **RED**: Write a pgtest: `SET ROLE some_role; SELECT pg_tviews_create(...)`. Assert
  the audit row's `performed_by` is the original session user, not `some_role`.
- **GREEN**: Replace `SELECT current_user::text` with `SELECT session_user::text` in all
  three audit functions (or record both: `performed_by = session_user`,
  `effective_role = current_user`).
- **REFACTOR**: Extract a shared `fn current_session_user() -> spi::Result<String>` to
  avoid repeating the query in three places.
- **CLEANUP**: Clippy clean; update any audit table schema comments if they say
  "current_user".

### Cycle 4: F-05 — PREPARE TRANSACTION silently drops refresh queue (queue/xact.rs:349, hooks.rs:127-131)

**ROOT CAUSE:**
`handle_prepare()` is annotated `#[allow(dead_code)]` and is never called. The
ProcessUtility hook captures the GID for `PREPARE TRANSACTION` but does nothing with the
pending queue. `tview_xact_callback` receives `XACT_EVENT_PREPARE`, calls
`reset_metrics()`, and silently discards the queue. TVIEWs become stale with no warning.

**Decision for 0.1.0:** Emit `error!()` — completing 2PC is tracked for a future release.

- **RED**: Write a pgtest: begin a transaction, INSERT into a base table (enqueuing a
  refresh), then `PREPARE TRANSACTION 'gid'`. Assert it raises an error mentioning
  "2PC not supported" rather than succeeding silently.
- **GREEN**: In the ProcessUtility hook, when intercepting `PREPARE TRANSACTION`, check
  if the queue is non-empty. If so, call `error!("pg_tviews: PREPARE TRANSACTION is not
  supported when TVIEW refreshes are pending; commit or rollback first")`. If the queue
  is empty, allow the PREPARE to proceed normally.
- **REFACTOR**: Remove or clearly tombstone the `handle_prepare()` dead code and the
  GID-capture logic in the hook (since we're rejecting it). Add a comment pointing to
  the GitHub issue tracking full 2PC support.
- **CLEANUP**: Remove `#[allow(dead_code)]` from `handle_prepare`; either delete or
  convert to `todo!()` with a tracking comment.

### Cycle 5: F-06 — manual JSONB escaping in reconstruct_as_tview (ddl/convert.rs:366-378)

**ROOT CAUSE:**
```rust
let escaped_data = data.replace('\'', "''");
values.push(format!("('{id}'::uuid, '{escaped_data}'::jsonb)"));
```
Comment says "using quote_literal for safety" but uses manual `replace`. This is wrong
when `standard_conforming_strings = off` (backslash sequences active). UUID `id` is safe
by domain constraints but relying on it silently is fragile.

- **RED**: (Hard to unit-test server config.) Write a code review test: grep for
  `replace('\'` in convert.rs and assert the grep returns empty (i.e., the code no
  longer uses this pattern).
- **GREEN**: Replace the VALUES-string approach with individual parameterized SPI INSERT
  statements per backup row:
  ```rust
  for (id, data) in rows {
      Spi::run_with_args(
          "INSERT INTO backup_table (id, data) VALUES ($1::uuid, $2::jsonb)",
          &[
              DatumWithOid::new(id, UUIDOID),
              DatumWithOid::new(data, JSONBOID),
          ],
      )?;
  }
  ```
- **REFACTOR**: If the batch-values approach was used for performance (bulk insert),
  consider keeping a multi-row parameterized insert but using proper parameter binding.
- **CLEANUP**: Delete the misleading comment; clippy clean.

### Cycle 6: F-11 — unquoted identifiers in pg_tviews_refresh (admin.rs:88-93)

**ROOT CAUSE:**
```rust
let col_list = view_columns.join(", ");
Spi::run(&format!("TRUNCATE {tv_name}"))?;
Spi::run(&format!("INSERT INTO {tv_name} ({col_list}) SELECT {col_list} FROM {view_name}"))?;
```
Column names from `pg_attribute.attname` are joined raw — reserved words fail at runtime.
`tv_name` and `view_name` are also unquoted.

- **RED**: Write a pgtest that creates a TVIEW with a column named `"order"` (reserved
  word) and calls `pg_tviews_refresh(...)`. Assert it succeeds without SQL syntax error.
- **GREEN**:
  ```rust
  let col_list = view_columns.iter()
      .map(|c| quote_identifier(c))
      .collect::<Vec<_>>()
      .join(", ");
  let qi_tv = quote_identifier(&tv_name);
  let qi_view = quote_identifier(&view_name);
  Spi::run(&format!("TRUNCATE {qi_tv}"))?;
  Spi::run(&format!("INSERT INTO {qi_tv} ({col_list}) SELECT {col_list} FROM {qi_view}"))?;
  ```
- **REFACTOR**: No structural change needed.
- **CLEANUP**: Confirm `quote_identifier` import path is consistent with Phase 3's
  consolidation (if Phase 3 ran first, use the consolidated version).

## Dependencies

- Requires: Phase 1 complete (hook is now safe to modify)
- Blocks: Phase 3

## Status

[x] Complete

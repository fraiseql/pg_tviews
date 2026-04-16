# pg_tviews 0.1.0 — Security & Quality Audit Report

**Date:** 2026-04-16T21:49:44Z  
**Auditor:** Claude Sonnet 4.6 (claude-sonnet-4-6)  
**Codebase:** `/home/lionel/code/pg_tviews` @ commit `709d517`  
**pgrx version:** 0.17.0  
**PostgreSQL target:** PG18

---

## Scope

Full `src/` tree. Audit plan from `/tmp/pg_tviews_audit.md` (8 sections). Files read:
`src/lib.rs`, `src/hooks.rs`, `src/queue/xact.rs`, `src/trigger.rs`, `src/catalog.rs`,
`src/refresh/main.rs`, `src/refresh/bulk.rs`, `src/queue/mod.rs`, `src/queue/state.rs`,
`src/queue/ops.rs`, `src/audit.rs`, `src/admin.rs`, `src/ddl/mod.rs`, `src/ddl/create.rs`,
`src/ddl/convert.rs`, `src/dependency/triggers.rs`, `src/schema/analyzer.rs`,
`src/lifecycle.rs`, `src/utils.rs`, `src/validation.rs`

---

## Finding F-01 · HIGH · Section 1 (Memory Safety / FFI)

**`HOOK_IN_PROGRESS` permanently set after `error!()` longjmps from inside `catch_unwind` closure**

**Location:** `src/hooks.rs:88, 205`

**Description:**
`tview_process_utility_hook` sets `HOOK_IN_PROGRESS = true` at line 88 to prevent re-entrant
hook processing. A `HOOK_IN_PROGRESS = false` reset at line 205 is intended to clear the guard
after the hook finishes. However, when `error!()` is called from *inside* the `catch_unwind`
closure (lines 112–169), it triggers `ereport(ERROR)` → `siglongjmp`. That longjmp jumps
directly to the `setjmp` in pgrx's `pg_guard_ffi_boundary`, bypassing all Rust code between the
call site and the FFI boundary — including both `catch_unwind` (which only catches Rust panics,
not C longjmps) and the final `HOOK_IN_PROGRESS = false` on line 205.

Both `handle_create_table_as` (called inside the closure at line 153) and `handle_drop_table`
(line 162) contain multiple `error!()` call sites:

- `handle_create_table_as`: entity name empty check, null query string, pattern not found, store
  failure, parse failure
- `handle_drop_table`: non–IF EXISTS drop failure

After any of these errors, `HOOK_IN_PROGRESS` remains `true` for the remainder of the backend
session. Every subsequent utility statement — DDL or otherwise — enters the guard at line 84 and
immediately returns without hook processing. TVIEW creation and DROP TABLE interception are
silently disabled for that connection.

Note: the COMMIT flush path (lines 95–108) correctly resets `HOOK_IN_PROGRESS = false` *before*
calling `error!()`, so that path is safe.

**Proof sketch:**
```sql
CREATE TABLE tv_;  -- no entity name → entity_name.is_empty() → error!() inside catch_unwind
-- HOOK_IN_PROGRESS is now permanently true for this backend session
CREATE TABLE tv_post AS SELECT pk_post, ...;  -- silently passes through unintercepted
```

**Fix:** Replace every `error!(...)` inside `handle_create_table_as` and `handle_drop_table` with
`return Err(TViewError::...)`. Make both handlers return `Result<bool, TViewError>`. Handle the
`Err` case outside the `catch_unwind` block (in the existing `Err(panic_info)` branch or a new
error branch). This ensures `error!()` is only called after `HOOK_IN_PROGRESS = false` has been
reset. A Rust RAII guard is not sufficient here because longjmp bypasses Rust destructors.

---

## Finding F-02 · MEDIUM · Section 2 (SQL Injection)

**`entity_name` from schema inference used unparameterized in SQL format strings in `register_metadata`**

**Location:** `src/ddl/create.rs:452–464`

**Description:**
`pg_tviews_create(tview_name, select_sql)` in `ddl/mod.rs` correctly applies
`validate_sql_identifier(tview_name)` to the first parameter. However, the effective
`entity_name` used later is taken from `final_schema.entity_name` (line 70), which is extracted
by `infer_schema(select_sql)` from the user-supplied SELECT. If the SELECT contains a pk column
declared as a quoted identifier with SQL metacharacters (e.g., `"pk_user' OR '1'='1"`),
`entity_name` can contain those characters without triggering the validation.

`entity_name` propagates to `view_name = format!("v_{entity_name}")` which is then used
unescaped in:

```rust
// ddl/create.rs:452-464 (register_metadata)
Spi::get_one::<pg_sys::Oid>(&format!(
    "... WHERE c.relname = '{view_name}' AND n.nspname = '{schema_name}' ..."
))
```

A crafted entity_name containing a UNION clause could cause this query to return the OID of an
arbitrary relation instead of the actual view, persisting a wrong `view_oid` into `pg_tview_meta`.
Subsequent refreshes using that OID would query the wrong relation, causing incorrect refreshes
and data corruption.

**Proof sketch:** User calls `pg_tviews_create('post', 'SELECT "pk_post'' UNION SELECT ...", ...')`.
`infer_schema` extracts the quoted column content as entity_name, bypassing `validate_sql_identifier`.
`create_backing_view` succeeds (using `quote_identifier`), then `register_metadata` executes the
malformed SQL.

**Fix:** Add `validate_sql_identifier(entity_name, "entity_name")?` immediately after line 70 in
`create_tview`, before any use of `entity_name`. Also replace the two unparameterized
`Spi::get_one` calls in `register_metadata` with parameterized queries:

```rust
Spi::get_one_with_args::<pg_sys::Oid>(
    "SELECT c.oid FROM pg_class c \
     JOIN pg_namespace n ON c.relnamespace = n.oid \
     WHERE c.relname = $1 AND n.nspname = $2 AND c.relkind = 'v'",
    &[
        unsafe { DatumWithOid::new(view_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        unsafe { DatumWithOid::new(schema_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
    ],
)
```

---

## Finding F-03 · MEDIUM · Section 2 (SQL / Correctness)

**Multi-table `DROP TABLE tv_a, tv_b` only processes first `tv_*` name; standard handler suppressed for all**

**Location:** `src/hooks.rs:407–445`

**Description:**
`handle_drop_table` splits the raw query string on whitespace and stops at the *first* word
starting with `tv_`. For `DROP TABLE tv_foo, tv_bar`, only `tv_foo` is processed. The hook
returns `true` (handled), which suppresses `call_prev_hook_or_standard` for the entire statement.
`tv_bar` is never processed by `drop_tview`: its metadata and triggers are orphaned.

Additionally, `DROP TABLE some_regular_table, tv_foo` would process `tv_foo` via `drop_tview`,
return `true`, and never invoke the standard DROP — leaving `some_regular_table` un-dropped,
silently.

**Fix:** Collect *all* `tv_*` words from the statement and call `drop_tview` for each. If
non-`tv_*` tables are also listed, call `call_prev_hook_or_standard` for those. Alternatively,
traverse the parsed `DropStmt.objects` list (available via the `drop_stmt` pointer already passed
to the function) instead of doing raw string splitting.

---

## Finding F-04 · MEDIUM · Section 3 (Privilege Escalation)

**`current_user` in audit log is spoofable via `SET ROLE`**

**Location:** `src/audit.rs:6–7, 28–29, 47–48`

**Description:**
All three audit functions (`log_create`, `log_drop`, `log_refresh`) record `performed_by` using
`SELECT current_user::text`. `current_user` reflects the *active role* after `SET ROLE`, not
the original authenticated identity. A session user `alice` can do:

```sql
SET ROLE admin;
SELECT pg_tviews_create('post', '...');  -- audit log records performed_by = 'admin'
```

The audit log then contains a false attribution, undermining its value as a security trail. For
defensive or compliance purposes, `session_user` (the original authenticated user) is the
correct value.

**Fix:** Replace `SELECT current_user::text` with `SELECT session_user::text` in all three audit
functions, or record both columns: `performed_by = session_user`, `effective_role = current_user`.

---

## Finding F-05 · MEDIUM · Section 5 (Transaction Safety)

**2PC queue persistence (`handle_prepare`) is dead code — `PREPARE TRANSACTION` silently drops pending refreshes**

**Location:** `src/queue/xact.rs:349–386`, `src/hooks.rs:517`

**Description:**
`handle_prepare()` at `queue/xact.rs:349` is annotated `#[allow(dead_code)]`. The ProcessUtility
hook captures the GID when intercepting `PREPARE TRANSACTION` (hooks.rs:127–131), but nothing
ever calls `handle_prepare()` to serialize the queue to `pg_tview_pending_refreshes`. When a 2PC
transaction prepares, `tview_xact_callback` receives `XACT_EVENT_PREPARE`, calls
`reset_metrics()`, and discards the queue without persisting it. On `COMMIT PREPARED`, no
refreshes are executed.

Users of 2PC observe: base-table changes committed via 2PC do not trigger TVIEW refreshes.
There is no error or warning. The TVIEW data silently becomes stale.

**Fix:** Either (a) complete the 2PC implementation by calling `handle_prepare()` from the
ProcessUtility hook when intercepting `PREPARE TRANSACTION`, or (b) detect `PREPARE TRANSACTION`
and emit `error!()` with "pg_tviews: 2PC not yet supported". Option (b) is safer for 0.1.0.
Silently dropping the queue is the worst outcome.

---

## Finding F-06 · MEDIUM · Section 2 (SQL Injection)

**Manual JSONB escaping in `reconstruct_as_tview` misleads and fails on non-default server config**

**Location:** `src/ddl/convert.rs:366–378`

**Description:**
```rust
// Non-empty table: reconstruct with actual data using quote_literal for safety
let escaped_data = data.replace('\'', "''");
values.push(format!("('{id}'::uuid, '{escaped_data}'::jsonb)"));
```

The comment says "using quote_literal for safety" but the code performs manual
`replace('\'', "''")` instead of calling PostgreSQL's `quote_literal()`. The manual escape is
correct for `standard_conforming_strings = on` (the default since PG 9.1) but is incorrect when
`standard_conforming_strings = off`, where backslash sequences are active. A JSONB value
containing `\''` on a non-default server could produce broken SQL.

Additionally, `id` (UUID) is interpolated with no escaping at all. UUID values from PostgreSQL's
`uuid` type are constrained to `[0-9a-f-]`, so no injection is possible via `id` — but relying
on this implicit constraint without a comment is fragile.

**Fix:** Replace the manual escaping with individual parameterized SPI INSERT statements per
backup row, which eliminates both the escaping concern and the misleading comment. If the
VALUES-string approach must be kept, call `quote_literal($1)` via SPI rather than escaping
manually.

---

## Finding F-07 · LOW · Section 1 (Memory Safety)

**`SAVEPOINT_DEPTH` underflows to `usize::MAX` in release mode on mismatched subxact events**

**Location:** `src/queue/xact.rs:144, 157`

**Description:**
Both `SUBXACT_EVENT_ABORT_SUB` and `SUBXACT_EVENT_COMMIT_SUB` handlers unconditionally decrement
`SAVEPOINT_DEPTH`:
```rust
let mut depth = d.borrow_mut();
*depth -= 1;
```

`SAVEPOINT_DEPTH` is a `usize`. If PostgreSQL delivers an `ABORT_SUB` or `COMMIT_SUB` without a
prior `START_SUB` (possible if the extension loads mid-transaction, or on edge-case event
ordering), this underflows. In debug builds: panic, caught by `catch_unwind`, emits `warning!`.
In release builds: silent wraparound to `usize::MAX`, permanently invalidating the depth counter
and causing `QUEUE_SNAPSHOTS` pop/push to be mismatched forever.

**Fix:**
```rust
*depth = depth.saturating_sub(1);
```
Add a `warning!` when the pre-decrement value is already 0 to surface the ordering bug.

---

## Finding F-08 · LOW · Section 8 (Code Quality)

**pgrx 0.16.1 workaround in `entity_for_table_uncached` unchanged on 0.17.0**

**Location:** `src/catalog.rs:414–430`

**Description:**
The comment at line 414 states:
```
// NOTE: Uses Spi::connect + client.select instead of Spi::get_one_with_args
// because pgrx 0.16.1's get_one_with_args errors (SpiTupleTable position)
// when the query returns zero rows.
```

The project now uses pgrx `0.17.0`. If the bug is fixed, the workaround adds unnecessary
complexity. If it still exists in 0.17.0, the comment should say so explicitly and a test should
be added to prevent regression.

**Fix:** Test `Spi::get_one_with_args` with a zero-row result in pgrx 0.17.0. If fixed, simplify
to a `Spi::get_one_with_args` one-liner. If not fixed, update the comment to reference the
specific pgrx issue number.

---

## Finding F-09 · LOW · Section 4 (DoS / Resource Exhaustion)

**Refresh queue is unbounded in memory; bulk DML can exhaust backend memory**

**Location:** `src/queue/state.rs:12`, `src/queue/ops.rs:9–37`

**Description:**
The transaction-local queue `TX_REFRESH_QUEUE` is an unsized `HashSet<RefreshKey>`. A single
bulk DML statement (`INSERT INTO tb_post SELECT * FROM large_table`) fires the row-level trigger
once per row. With N unique PKs, the queue grows to N entries. `max_propagation_depth` caps
propagation *iterations* but has no effect on the initial queue population. No backpressure
mechanism exists.

Any user with INSERT/UPDATE/DELETE on a base table that has a TVIEW trigger installed can fill
backend memory by running bulk DML, crashing the backend (OOM) or degrading the server for other
connections.

**Fix:** Add a GUC `pg_tviews.max_queue_size` (default e.g. 100,000). Check the bound on every
`enqueue_refresh` call and call `error!()` with a clear message when exceeded.

---

## Finding F-10 · LOW · Section 1 / Section 4

**Regex patterns compiled on every `PREPARE TRANSACTION` invocation**

**Location:** `src/hooks.rs:497–508`

**Description:**
```rust
fn extract_gid_from_prepare_query(query: &str) -> Option<String> {
    let patterns = [
        "PREPARE\\s+TRANSACTION\\s+'([^']+)'",
        "PREPARE\\s+TRANSACTION\\s+\"([^\"]+)\"",
    ];
    for pattern in &patterns {
        if let Ok(re) = regex::Regex::new(pattern) ...
```

Two `Regex::new` calls execute on every `PREPARE TRANSACTION` statement, inside the ProcessUtility
hook. Regex compilation is not free. The patterns themselves use negated character classes
(`[^']+`, `[^"]+`) and cannot catastrophically backtrack — this is not a safety issue, but it is
unnecessary allocation per statement.

**Fix:** Compile the regexes exactly once using `std::sync::LazyLock`:
```rust
static GID_RE_SINGLE: LazyLock<Regex> = LazyLock::new(|| 
    Regex::new(r"PREPARE\s+TRANSACTION\s+'([^']+)'").unwrap()
);
static GID_RE_DOUBLE: LazyLock<Regex> = LazyLock::new(|| 
    Regex::new(r#"PREPARE\s+TRANSACTION\s+"([^"]+)""#).unwrap()
);
```

---

## Finding F-11 · LOW · Section 7 (Error Handling)

**Column names and relation names unquoted in `pg_tviews_refresh` SQL**

**Location:** `src/admin.rs:88–93`

**Description:**
```rust
let col_list = view_columns.join(", ");
Spi::run(&format!("TRUNCATE {tv_name}"))?;
Spi::run(&format!(
    "INSERT INTO {tv_name} ({col_list}) SELECT {col_list} FROM {view_name}"
))?;
```

`col_list` is a comma-joined list of raw column names from `pg_attribute.attname`, without
`quote_identifier`. Column names that are PostgreSQL reserved words will cause a syntax error at
runtime. `tv_name` and `view_name` are also used unquoted. `quote_identifier` is available and
used consistently in all other DML/DDL in the codebase.

**Fix:**
```rust
let col_list = view_columns.iter()
    .map(|c| quote_identifier(c))
    .collect::<Vec<_>>()
    .join(", ");
let qi_tv = quote_identifier(&tv_name);
let qi_view = quote_identifier(&view_name);
Spi::run(&format!("TRUNCATE {qi_tv}"))?;
Spi::run(&format!(
    "INSERT INTO {qi_tv} ({col_list}) SELECT {col_list} FROM {qi_view}"
))?;
```

---

## Finding F-12 · LOW · Section 8 (Code Quality)

**Large 2PC persistence infrastructure and `log_refresh` are dead code without a removal plan**

**Location:** `src/audit.rs:46`, `src/queue/xact.rs:349,391`, `src/hooks.rs:517`,
`src/queue/persistence.rs:34,72,81,90,114,133,143`

**Description:**
Beyond `handle_prepare` (noted in F-05), seven items in `queue/persistence.rs` are annotated
`#[allow(dead_code)]`. `audit.rs::log_refresh` is never called from any refresh path. Prepared-
statement cache infrastructure in `refresh/cache.rs` and several metrics fields are also
annotated as "not yet wired". For a 0.1.0-beta release, accumulating unfinished infrastructure
as dead code creates maintenance confusion and can mask real dead-code warnings.

**Fix:** Before 0.1.0: either wire the features (completing them and removing the
`#[allow(dead_code)]` annotations) or delete the dead code entirely. Each `#[allow(dead_code)]`
currently in the codebase represents technical debt that compounds.

---

## Finding F-13 · INFO · Section 5 (Transaction Safety)

**`refresh_pk` errors on missing row; savepoint restore likely prevents this, but worth hardening**

**Location:** `src/refresh/main.rs:267–272`

**Description:**
`recompute_view_row` returns `Err("No row in v_* for given pk: {pk}")` when the backing view
returns no rows. This error propagates to `flush_refresh_queue`, then to the COMMIT hook's
`error!()`, aborting the transaction. If a key for a deleted row survives to flush time, the
entire COMMIT fails.

**Assessment:** On `ROLLBACK TO SAVEPOINT`, the queue is replaced with the pre-savepoint snapshot
(`replace_queue`), removing keys that were enqueued within the savepoint. For rows deleted via a
rolled-back savepoint, the key would be removed. The risk scenario — a key surviving for a row
that is truly gone at flush time — requires either a DELETE that commits successfully, in which
case the TVIEW row should also be deleted (handled by the UPSERT-on-conflict path), or an edge
case involving TRUNCATE within a savepoint.

**Fix (optional hardening):** In `recompute_view_row`, treat a missing row as a DELETE signal
rather than an error. Return `Ok(None)` and let `refresh_and_get_parents` call a delete on the
TVIEW row. This makes the flush idempotent for deletes and eliminates the COMMIT-abort risk.

---

## Finding F-14 · INFO · Section 1 (Memory Safety / FFI)

**`catch_unwind` in `tview_subxact_callback` is correct as implemented**

**Location:** `src/queue/xact.rs:127–179`

**Description:**
The audit plan asks whether `catch_unwind` in the subxact callback risks catching a pgrx
longjmp-to-panic conversion, corrupting `PG_exception_stack`. After review: the code inside the
`AssertUnwindSafe` closure consists entirely of thread-local `RefCell` accesses and pure Rust
`HashSet` operations. No SPI calls, no pgrx error macros, no FFI that could trigger `ereport()`
or a longjmp. Therefore, only genuine Rust panics (e.g., `RefCell` double-borrow, `usize`
underflow — see F-07) can reach `catch_unwind`. These are correctly handled without touching
`PG_exception_stack`.

**Status:** No action needed on the `catch_unwind` itself.

---

## Finding F-15 · INFO · Section 2 (SQL Injection)

**OID interpolation via `{oid:?}` is safe throughout the codebase**

**Location:** `src/catalog.rs:397`, `src/dependency/triggers.rs:200`, `src/refresh/bulk.rs:148`,
`src/admin.rs:104–106`, and others

**Description:**
Several functions interpolate OIDs directly into SQL via `format!("... WHERE oid = {oid:?}")`.
`pg_sys::Oid` is `u32`. Its `Debug` format emits a decimal integer (e.g., `16384`). This cannot
contain SQL metacharacters. All OIDs in use are sourced from trigger callbacks, SPI OID lookups,
or `pg_tview_meta` — they are never derived from raw user input. No injection risk exists via
this pattern.

**Status:** No action needed.

---

## Finding F-16 · INFO · Section 2 (SQL Injection)

**`build_smart_patch_sql` dependency path values are regex-captured `\w+` — no injection**

**Location:** `src/refresh/main.rs:467–490`, `src/schema/analyzer.rs:153–157`

**Description:**
`path_str` and `match_key` are interpolated unquoted into `ARRAY['{path_str}']` and
`'{match_key}'` SQL literals. These values originate from `dependency_paths` and
`array_match_keys` in `pg_tview_meta`, populated by `analyze_dependencies` via regexes with
`(\w+)` capture groups. `\w+` matches only `[A-Za-z0-9_]`, excluding `'`, `;`, and other SQL
metacharacters. The captured values cannot contain SQL injection payloads.

**Status:** No action needed. The regex constraint is the correct and sufficient protection here.

---

## Summary Table

| # | Severity | Section | Location | One-line description |
|---|----------|---------|----------|----------------------|
| F-01 | **HIGH** | §1 FFI | `hooks.rs:88,205` | `HOOK_IN_PROGRESS` leaked when `error!()` longjmps from inside `catch_unwind` |
| F-02 | **MEDIUM** | §2 SQL | `ddl/create.rs:452-464` | `entity_name` from schema inference not validated; unparameterized in `register_metadata` SQL |
| F-03 | **MEDIUM** | §2 SQL | `hooks.rs:407-445` | `DROP TABLE tv_a, tv_b` processes only first name; standard handler suppressed for all |
| F-04 | **MEDIUM** | §3 Privilege | `audit.rs:6-7` | `current_user` in audit log spoofable via `SET ROLE`; use `session_user` |
| F-05 | **MEDIUM** | §5 Tx Safety | `queue/xact.rs:349` | `handle_prepare()` is dead code; `PREPARE TRANSACTION` silently drops refresh queue |
| F-06 | **MEDIUM** | §2 SQL | `ddl/convert.rs:366-378` | Manual JSONB escaping misleads (comment wrong) and fails on `standard_conforming_strings=off` |
| F-07 | LOW | §1 Memory | `queue/xact.rs:144,157` | `SAVEPOINT_DEPTH` usize underflows to `usize::MAX` in release on mismatched subxact events |
| F-08 | LOW | §8 Quality | `catalog.rs:414` | pgrx 0.16.1 workaround comment unchanged on pgrx 0.17.0; may be dead complexity |
| F-09 | LOW | §4 DoS | `queue/state.rs:12` | Refresh queue has no size cap; bulk DML can exhaust backend memory |
| F-10 | LOW | §1/§4 | `hooks.rs:497-508` | Two regexes compiled on every `PREPARE TRANSACTION`; should be `LazyLock` |
| F-11 | LOW | §7 Error | `admin.rs:88-93` | Column and relation names unquoted in `pg_tviews_refresh` SQL |
| F-12 | LOW | §8 Quality | Multiple | Substantial 2PC persistence infra and `log_refresh` are dead code without removal plan |
| F-13 | INFO | §5 Tx Safety | `refresh/main.rs:267` | `refresh_pk` errors on missing row; savepoint restore likely prevents, but consider hardening |
| F-14 | INFO | §1 FFI | `queue/xact.rs:127` | `catch_unwind` in subxact callback is correct — no SPI or pgrx macros inside the closure |
| F-15 | INFO | §2 SQL | `catalog.rs:397` | OID `{:?}` interpolation is safe (u32 decimal, always catalog-sourced) |
| F-16 | INFO | §2 SQL | `refresh/main.rs:467-490` | Smart-patch path values captured via `\w+` — no injection possible |

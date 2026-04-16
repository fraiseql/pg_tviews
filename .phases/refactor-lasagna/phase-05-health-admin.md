# Phase 5: Health Check and Admin Cleanup

## Objective

Replace the 5 hardcoded sequential check blocks in `src/health.rs` with a
small data-driven pattern, and consolidate the 2 identical error-to-JSONB
wrappers in `src/admin.rs`.

## Background

### `health.rs`

`pg_tviews_health_check` (health.rs:22–111) performs 5 checks. Each check is
a free-standing block:

```rust
// Check N: <description>
let value = Spi::get_one::<T>(...).unwrap_or(...).unwrap_or(default);
if condition {
    results.push(("OK"/"WARNING"/"ERROR", "component", "msg", "severity"));
} else {
    results.push((...));
}
```

Adding a sixth check requires copy-pasting this structure. The component
name, SQL, and pass/fail messages are the only varying parts.

### `admin.rs`

`pg_tviews_analyze_select` (lines 14–28) has a nested match with two
separate `JsonB(serde_json::json!({"error": ...}))` arms — one for
`infer_schema` failure and one for serialization failure. This can be
flattened with `and_then`.

## Success Criteria

- [ ] `health.rs` uses a `HealthCheck` struct + a single results-building loop
      (or equivalent data-driven approach)
- [ ] Adding a future check requires only adding a new data entry, not a new
      code block
- [ ] `pg_tviews_analyze_select` in `admin.rs` uses a flat `and_then` chain
      instead of nested matches
- [ ] All checks still return the same status/component/message/severity values
- [ ] Clippy clean; tests pass

## TDD Cycles

### Cycle 1: Data-driven health checks (GREEN → REFACTOR)

No behaviour change; no RED phase.

**GREEN**

Define a small private type that captures a check's varying parts and a
uniform evaluation function:

```rust
struct HealthCheck {
    component: &'static str,
    /// SQL that returns a single value (bool or i64)
    sql: &'static str,
    /// Interpret the query result and return (status, message, severity)
    evaluate: fn(Option<pgrx::datum::AnyNumeric>) -> (&'static str, String, &'static str),
}
```

Alternatively, use closures if the check logic is varied enough to warrant it.

For the simpler boolean checks (check 2: jsonb_delta, check 4: orphaned
triggers) a minimal enum works cleanly:

```rust
enum CheckKind {
    /// bool query: true = OK, false = WARNING
    BoolCheck { ok_msg: &'static str, warn_msg: &'static str },
    /// count query: 0 = OK, >0 = error with count
    CountCheck { ok_msg: &'static str, severity_if_nonzero: &'static str },
    /// Always OK — no query needed, just report a value
    InfoOnly { message_fn: fn() -> String },
}
```

Build the check table as a `const` or `static` slice. The main function
becomes a loop over the table.

**REFACTOR**

Assess which approach is cleaner given the actual checks:

- Check 1 is `InfoOnly` (no SQL, just version string)
- Check 2 is `BoolCheck` (jsonb_delta)
- Check 3 is `CountCheck` with ERROR severity
- Check 4 is `CountCheck` with WARNING severity
- Check 5 is `InfoOnly` (count, always OK)

If the closures become awkward due to `pgrx`'s non-`Send`/non-`Sync` SPI
context, fall back to a simpler `fn run_check(check: &CheckKind) -> (...)` that
matches on the enum variant and executes inline — still better than 5 separate
blocks.

**CLEANUP**

- Verify the output order of checks is preserved (the caller may depend on
  row ordering if used in monitoring scripts)
- Run `cargo clippy` and `cargo test`
- Commit: `refactor(health): replace 5 hardcoded check blocks with data-driven loop`

---

### Cycle 2: Flatten `pg_tviews_analyze_select` (GREEN → REFACTOR)

Note: `pg_tviews_infer_types` uses `error!()` (hard PostgreSQL ERROR) on
failure — intentionally different from `analyze_select`'s soft-error JSONB
pattern. Do **not** unify them; they have different error semantics by design.

**GREEN**

Flatten the nested match in `pg_tviews_analyze_select` into a single
`and_then` chain:

```rust
#[pg_extern]
fn pg_tviews_analyze_select(sql: &str) -> JsonB {
    match crate::schema::inference::infer_schema(sql)
        .map_err(|e| e.to_string())
        .and_then(|s| s.to_jsonb().map_err(|e| format!("Failed to serialize schema: {e}")))
    {
        Ok(jsonb) => jsonb,
        Err(msg) => JsonB(serde_json::json!({"error": msg})),
    }
}
```

**CLEANUP**

- Run `cargo clippy` and `cargo test`
- Commit: `refactor(admin): flatten nested match in pg_tviews_analyze_select`

## Files Touched

- `src/health.rs`
- `src/admin.rs`

## Risk

Low. Both files are purely output/display logic with no side effects on
PostgreSQL state. The only observable output is the returned rows/JSONB, which
must be identical before and after.

**Validation**: Before starting, capture the output of
`SELECT * FROM pg_tviews_health_check()` in the test database. After the
refactor, verify the output is byte-identical.

# 🏛️ **TVIEW Extension — Architecture**

The TVIEW extension (“`pg_tview`”) transforms PostgreSQL into a **real-time read model engine** for FraiseQL by:

* Automatically materializing `v_*` views into `tv_*` tables
* Incrementally updating those tables on `tb_*` table changes
* Propagating changes upward through TVIEW dependencies
* Supporting efficient PK-based lineage & UUID-based filtering

Everything is deterministic, synchronous, and perfectly aligned with GraphQL Cascade.

---

# 📘 1. High-Level Data Flow

### GraphQL Mutation:

```
UUID input → FraiseQL resolves to PK → writes to tb_*
```

### TVIEW Extension:

```
AFTER UPDATE trigger on tb_* 
    → recompute v_entity WHERE pk = X
    → patch tv_entity via jsonb_delta
    → propagate to parent tv_* using FK columns
```

### GraphQL Cascade:

```
FraiseQL queries tv_* (UUID-based filtering)
→ returns updated nested JSONB read models
```

The system now acts like a **reactive relational graph**.

---

# 🧱 2. The TVIEW Triple-Layer Model

```
tb_entity      → normalized write model
v_entity       → pure SQL “read-model definition”
tv_entity      → materialized + incrementally updated read model
```

### Developer workflow:

1. Define `v_entity` as a **SQL view** exposing:

   * `pk_entity`
   * `id` (UUID)
   * all **FK columns** (PK & UUID only where needed)
   * read model JSONB (`data`)

2. Register TVIEW:

```sql
CREATE TABLE tv_entity AS SELECT * FROM v_entity;
```

3. TVIEW engine auto-creates:

   * `tv_entity` physical table
   * triggers on underlying `tb_*` tables
   * refresh pipeline

---

# 🏗️ 3. TVIEW Table Schema (Standardized)

For entity `post`:

```sql
CREATE TABLE tv_post (
  pk_post    integer primary key,  -- lineage root
  id         uuid not null,        -- for GraphQL
  fk_user    integer not null,     -- lineage FK
  user_id    uuid not null,        -- filtering FK for FraiseQL
  data       jsonb not null,       -- read model
  updated_at timestamptz not null
);
```

### Key principles:

* **PK integer** for lineage
* **FK integer(s)** for propagation
* **UUID id** for external exposure
* **UUID FK(s)** *only where filtering is needed*
* **JSONB** as full read model

This solves:

* GraphQL filtering
* DB lineage
* FraiseQL input/output

---

# 🧠 4. Lineage Engine (Core Logic)

Because we have PK/FK integer columns:

### Lineage resolution is trivial:

When a `tb_post` row changes:

1. Update corresponding `tv_post` row.
2. Using `fk_user`, update `tv_user` row.
3. Using `fk_company`, update `tv_company` row (if defined).
4. Continue until root of TVIEW DAG is reached.

No JSON introspection.
No dependency guesswork.
No row-level lineage table needed.

This is the *magic* that makes TVIEW viable.

---

# ⚙️ 5. Update Pipeline (Synchronous)

### Step 1 — Mutation writes to `tb_post`

After FraiseQL resolves UUID → PK mapping.

### Step 2 — Trigger fires

Rust trigger receives:

```rust
source_oid = tb_post::oid
pk = NEW.pk_post
```

### Step 3 — TVIEW recomputes view fragment

Rust executes:

```sql
SELECT pk_post, id, fk_user, user_id, data
FROM v_post
WHERE pk_post = $1;
```

### Step 4 — TVIEW patches the materialized row

```sql
UPDATE tv_post
SET data = jsonb_delta_patch(data, $new.data),
    updated_at = now(),
    user_id = $new.user_id,
    fk_user = $new.fk_user
WHERE pk_post = $1;
```

### Step 5 — Propagate

Rust queries:

```sql
SELECT v_parent.pk_parent
FROM tv_post
JOIN ... -- using FK columns
```

Then:

* recompute `v_user WHERE pk_user = X`
* patch `tv_user`
* propagate further if needed

This is **fast** because:

* lineage uses integers
* view recomputation is scoped to ONE pk
* patching is incremental

---

# 🧩 6. Dependency Graph — Using v_* Dependencies

TVIEWs don’t define dependencies themselves.

### Instead:

TVIEW follows **PostgreSQL's view dependency graph** for `v_*` views.

This is far simpler:

* No custom parsing
* No custom DSL
* No hidden magic

For each TVIEW we can query:

```sql
SELECT referenced_objects
FROM pg_depend 
JOIN pg_rewrite
WHERE view_oid = v_entity_oid;
```

Thus TVIEW discovers:

```
v_post depends on tb_post
v_user depends on v_post and tb_user
```

Hence:

```
tv_post → tv_user propagation chain
```

This uses built-in PostgreSQL capabilities.
Elegant and reliable.

---

# 📦 7. TVIEW System Catalog

We need minimal metadata:

### `pg_tview_meta`

| column     | description                   |
| ---------- | ----------------------------- |
| tview_oid  | OID of `tv_entity` table      |
| view_oid   | OID of `v_entity` view        |
| entity     | text name (“user”, “post”, …) |
| sync_mode  | 'sync' (default) or 'async'   |
| created_at | timestamp                     |

### That’s it.

### Why so small?

Because:

* Lineage = FK columns
* Dependencies = PostgreSQL dependency tree
* JSONB patching = delegated to jsonb_delta
* View logic = stored in PostgreSQL views

TVIEW does not reinvent anything.

---

# 🦾 8. Rust Implementation Overview

### Modules:

```
src/
 ├ catalog.rs       -- pg_tview_meta support
 ├ trigger.rs       -- sync update trigger for tb_*
 ├ refresh.rs       -- view recompute + jsonb_delta patch
 ├ propagate.rs     -- lineage propagation via FK columns
 ├ util.rs
 └ lib.rs           -- extension entrypoint
```

### Trigger (Rust)

```rust
#[pg_trigger]
fn tview_after_change(trigger: &PgTrigger) -> ... {
    let (rel_oid, old, new) = (...) ;
    let pk = extract_pk(new.or(old));
    tview_refresh_row(rel_oid, pk)?;
    Ok(new)
}
```

### Refresh (Rust)

```rust
fn tview_refresh_row(source_oid: Oid, pk: i64) {
    let view_row = recompute_view_fragment(source_oid, pk)?;
    apply_patch_to_tv(view_row)?;
    propagate_to_parents(view_row)?;
}
```

Everything is SPI-based.

---

# 🧬 9. Synchronous Update Semantics

Because GraphQL Cascade **expects immediate consistency**, TVIEW runs **inside the same transaction** as the mutation.

This is safe because:

* Each recompute is very small (one PK row)
* JSONB patching is incremental
* Lineage propagation is bounded (view DAG depth is small)

Your architecture is optimized for synchronous behavior.

---

# 🧹 10. What TVIEW **does not** do

* Does not allow `tv_*` to be updated directly
* Does not replace or modify `v_*`
* Does not manage UUID/PK conversion (FraiseQL does)
* Does not support arbitrary “multi-row” rebuilds beyond single-PK updates
* Does not overwrite JSON logic—view SQL remains source of truth

This keeps the extension small, reliable, and performant.

---

# 🎉 11. Summary — What the TVIEW Extension *Is*

### ✔ A synchronous, PK-driven incremental materialization engine

### ✔ A companion to FraiseQL and jsonb_delta

### ✔ An orchestrator that recomputes and patches `tv_*` tables

### ✔ A lineage-aware update propagator

### ✔ Built with Rust for safety and clarity

### ✔ Aligned with GraphQL Cascade semantics

---

# 🏗️ 12. Implementation Summary

- **Error types**: `TViewError` enum with 19 variants, SQLSTATE mapping, `TViewResult<T>`
- **Metadata**: `pg_tview_meta` and `pg_tview_helpers` tables, auto-created on load
- **Schema inference**: Column detection, type inference from PostgreSQL catalog
- **DDL hooks**: `ProcessUtility` hook for CREATE/DROP TABLE `tv_*` interception
- **Triggers**: Row-level change detection + statement-level flush triggers
- **Dependency graph**: Topological sort, cycle detection, cascade propagation
- **Queue system**: Transaction-scoped refresh queue with bulk optimization
- **Refresh engine**: Incremental JSONB patching, array aggregation support
- **2PC support**: Queue persistence for distributed transactions
- **Monitoring**: Metrics, health checks, performance views
- **Testing**: Rust unit tests, pgrx integration tests, SQL test suite, CI/CD pipeline

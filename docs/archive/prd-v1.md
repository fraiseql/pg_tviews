# 📘 **TVIEW Extension (pg_tview)**

## **Product Requirements Document + Technical Design Document**

**Version:** 1.1
**Status:** Design Complete
**Authors:** [You + ChatGPT]
**Language:** Rust (`pgrx`)
**Dependencies:** `jsonb_ivm` extension
**Target PostgreSQL Version:** 15+

---

# 1. Purpose and Context

Modern FraiseQL applications use a strict CQRS design:

```
tb_*  = normalized, transactional write models
v_*   = declarative read-model definitions (views)
tv_*  = materialized read models (tables)
```

Today, developers maintain `tv_*` via:

* manual recomputation logic
* triggers
* application-level glue
* scheduled rebuilds

This leads to:

* stale data
* inconsistent GraphQL responses
* duplicated JSON composition logic
* high engineering maintenance costs

The **TVIEW Extension** solves this by turning PostgreSQL into a **real-time, lineage-aware, synchronous read-model engine**.

It ensures:

* `tv_*` always matches the `v_*` definition
* updates propagate recursively through nested read models
* GraphQL Cascade always sees fresh data
* JSON computation logic lives in `v_*`, not in application code
* FraiseQL filtering remains UUID-only but efficient

TVIEW is **the missing infrastructure layer**.

---

# 2. Terminology

### **Trinity Identifier Model**

Every entity has:

* `id` (UUID): public identifier (FraiseQL/GraphQL)
* `pk_entity` (integer): primary key for lineage + joins
* `fk_*` (integer): foreign keys for lineage propagation
* `{parent}_id` (UUID): optional FK UUID for FraiseQL filtering

### **View Definitions**

`v_entity`: A SQL view that returns columns including:

* `pk_entity`
* `id` (UUID)
* all direct FK integers
* relevant UUID FKs (for FraiseQL filtering)
* recursively composed JSON read model via other `v_*` views

### **TVIEW Tables**

`tv_entity`: Materialized, incrementally updated tables mirroring the structure of `v_entity`.

TVIEW tables:

```
pk_entity       (int)
id              (uuid)
fk_parent       (int)
parent_id       (uuid)
data            (jsonb)
updated_at      (timestamp)
```

---

# 3. Goals

## 3.1 Functional Goals (FG)

| ID  | Goal                     | Description                                                               |
| --- | ------------------------ | ------------------------------------------------------------------------- |
| FG1 | Declarative read models  | Developers define `v_*` views; TVIEW derives `tv_*` tables                |
| FG2 | Materialized read models | `tv_*` tables store JSONB read models for fast GraphQL queries            |
| FG3 | Incremental updates      | Only affected rows recomputed and patched                                 |
| FG4 | Recursive propagation    | Changes propagate to parent TVIEWs according to view dependencies         |
| FG5 | Synchronous consistency  | TVIEW updates run inside write transaction (critical for GraphQL Cascade) |
| FG6 | Efficient filtering      | TVIEWs include UUID FK columns for FraiseQL filtering                     |
| FG7 | PK-based lineage         | TVIEW lineage uses integer PK/FK only                                     |
| FG8 | Developer-friendly       | No need to write triggers, ETL, or patch code                             |

---

## 3.2 Non-Goals (NG)

* TVIEWs are **not updatable** from application code
* Async mode is not included in v1
* TVIEW does not inspect or understand JSON semantics
* No distributed support (Citus / sharding) in v1
* Not intended as general-purpose incremental materialized view engine
* No magic inference of filters or recursive UUID flattening

---

# 4. Architectural Overview

## 4.1 System Flow

```
GraphQL Mutation
   |
   | UUID → PK resolution by FraiseQL
   v
tb_* change
   |
   | AFTER trigger
   v
TVIEW engine
   |
   | recompute (1 PK row)
   | patch JSON (via jsonb_ivm)
   | propagate to parents
   v
tv_* updated synchronously
   |
   v
GraphQL Cascade returns fresh view
```

---

## 4.2 Recursive View Composition (Core Principle)

Every read model view must assemble nested JSON by referencing other `v_*` views, **never** `tb_*` tables except for the root entity.

### Example

#### v_user:

```sql
CREATE VIEW v_user AS
SELECT
  u.pk_user,
  u.id,
  u.fk_company,
  c.id AS company_id,
  jsonb_build_object(
    'id', u.id,
    'name', u.name,
    'company', v_company.data
  ) AS data
FROM tb_user u
JOIN v_company ON v_company.pk_company = u.fk_company;
```

#### v_post:

```sql
CREATE VIEW v_post AS
SELECT
  p.pk_post,
  p.id,
  p.fk_user,
  u.id AS user_id,
  jsonb_build_object(
    'id', p.id,
    'title', p.title,
    'user', v_user.data
  ) AS data
FROM tb_post p
JOIN v_user ON v_user.pk_user = p.fk_user;
```

This creates a **declarative read-model DAG**:

```
v_company → v_user → v_post → v_comment
```

The TVIEW engine simply traverses this graph.

---

# 5. TVIEW Table Structure

For entity `post`:

```sql
CREATE TABLE tv_post (
  pk_post    integer primary key,
  id         uuid not null,
  fk_user    integer not null,
  user_id    uuid not null,
  data       jsonb not null,
  updated_at timestamptz not null
);
```

**Notes:**

* Only include FK UUIDs you want to filter on
* FK integers are for lineage
* JSON `data` nests deeper structures

---

# 6. Dependency Model (View-Based)

TVIEW uses PostgreSQL’s dependency catalog (`pg_depend`) to determine:

* which `v_*` depend on which other views
* which TVIEWs must be refreshed after a change

Example:

```
tb_user changes
↓
v_user depends on tb_user
↓
tv_user refresh
↓
v_post depends on v_user
↓
tv_post refresh
↓
v_feed depends on v_post
↓
tv_feed refresh
```

This is **automatic** and requires no developer configuration.

---

# 7. Update Pipeline (Technical Design)

## 7.1 Trigger on tb_* tables

TVIEW installs:

```sql
AFTER INSERT OR UPDATE OR DELETE ON tb_entity
FOR EACH ROW EXECUTE FUNCTION tview_trigger();
```

### Trigger Responsibilities:

* Extract `pk_entity`
* Call `tview_refresh_pk(entity, pk)`

---

## 7.2 Refresh Routine

### Step 1 — Recompute view row

Using SPI:

```sql
SELECT * FROM v_post WHERE pk_post = $pk;
```

### Step 2 — Patch TVIEW row using jsonb_ivm

```sql
UPDATE tv_post
SET data       = jsonb_ivm_patch(data, $new.data),
    fk_user    = $new.fk_user,
    user_id    = $new.user_id,
    updated_at = now()
WHERE pk_post = $pk;
```

### Step 3 — Propagate updates to parents

Use FK integers:

```sql
SELECT v_user.pk_user
FROM tb_post
WHERE pk_post = $pk;
```

Then call:

```
tview_refresh_pk(user, fk_user)
```

Repeat recursively according to `pg_depend`.

All inside the same transaction.

---

# 8. Rust Implementation (pgrx)

### 8.1 Directory Layout

```
src/
 ├ lib.rs
 ├ trigger.rs
 ├ refresh.rs
 ├ propagate.rs
 ├ catalog.rs
 └ util.rs
sql/
 └ pg_tview--1.0.sql
```

---

### 8.2 Example Trigger (Rust)

```rust
#[pg_trigger]
fn tview_trigger(trigger: &PgTrigger) -> TriggerResult {
    let pk = extract_pk(trigger)?;
    let source_table_oid = trigger.relation().oid();
    refresh_pk(source_table_oid, pk)?;
    Ok(trigger.new_or_old())
}
```

---

### 8.3 Refresh Function

```rust
pub fn refresh_pk(source_oid: Oid, pk: i64) -> spi::Result<()> {
    let row = recompute_view_row(source_oid, pk)?;
    apply_patch(&row)?;
    propagate(&row)?;
    Ok(())
}
```

---

### 8.4 Parent Propagation

```rust
pub fn propagate(row: &ViewRow) -> spi::Result<()> {
    for parent in parents_of(row.entity) {
        let fk = row.get_fk(parent.entity);
        refresh_pk(parent.source_oid, fk)?;
    }
    Ok(())
}
```

---

# 9. Transaction Guarantees

* TVIEW runs during the same transaction as the write
* If any refresh fails, **the mutation rolls back**
* Ensures GraphQL Cascade sees consistent read-model data immediately
* Supports optimistic UI patterns

---

# 10. Performance

### Strengths:

* PK/FK lineage → no row scanning
* Single-row view recompute → cheap
* JSONB patching (jsonb_ivm) → minimal diff writes
* Synchronous updates → no lag
* FraiseQL filtering uses UUID indexes → extremely fast

### Expected performance:

* 0.2–2 ms per TVIEW layer (typical)
* Recursive chains ≤ 4 levels remain < 10ms total

---

# 11. Error Handling

* Changing a `v_*` schema invalidates TVIEW → requires full rebuild
* Cyclic view dependencies detected on `CREATE TABLE tv_`
* Missing FK columns → `CREATE TABLE tv_` error
* JSON patch failure → fallback to full replacement
* Misconfigured FK UUIDs → validation error on creation

---

# 12. DDL (SQL Interface)

### Create

```sql
CREATE TABLE tv_post AS SELECT * FROM v_post;
```

### Drop

```sql
DROP TABLE tv_post;
```

### Manual Refresh

```sql
REFRESH TVIEW tv_post WHERE pk_post = 42;
```

---

# 13. Developer Constraints

To ensure TVIEW correctness:

### Required in v_entity

* Must output `pk_entity`
* Must join child entities via `v_child_entity`, **not tb_child_entity**
* Must provide direct FK integers
* Must include necessary UUID FKs for filtering
* Must output `data` containing assembled read model

---

# 14. Roadmap

| Milestone | Features                                                                                 |
| --------- | ---------------------------------------------------------------------------------------- |
| v1.0      | Synchronous updates, lineage propagation, view-based dependencies, jsonb_ivm integration |
| v1.5      | Background worker for optional async mode                                                |
| v2.0      | Automatic TVIEW table generation from view definition                                    |
| v2.5      | Distributed graph support                                                                |
| v3.0      | JSON path-level incremental lineage                                                      |

---

# 15. Final Summary

The **TVIEW extension** turns PostgreSQL into a fully reactive read-model engine:

* Developers only define `v_*` views
* TVIEW materializes them into `tv_*` tables
* Updates to `tb_*` propagate synchronously and incrementally
* JSON patches are efficient thanks to `jsonb_ivm`
* Lineage uses PK/FK integers for speed
* FraiseQL filters using UUID columns
* Composition of read models uses `v_*` → `v_*` joins

This architecture preserves:

* CQRS separation
* FraiseQL semantics
* GraphQL Cascade consistency
* PostgreSQL-native performance
* Maintainable declarative read models

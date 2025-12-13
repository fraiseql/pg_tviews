# 1. Scope of This Document

This document focuses on **how TVIEW handles multi-update transactions**:

* Many rows across many `tb_*` tables can change.
* The **same TVIEW row** might be logically affected multiple times.
* We want **exactly one refresh per `(entity, pk)` per transaction**, at **commit time**, in **dependency order**.

We assume the rest of TVIEW (CREATE TABLE tv_ AS SELECT, view/table generation, dependency graph) is already designed.

---

# 2. Requirements & Invariants

### R1 — Refresh coalescing

Within a single transaction:

* Each logical target `(entity_name, pk_value)` is refreshed **at most once**, regardless of how many triggers fired for it.

### R2 — End-of-transaction semantics

* TVIEW refreshes MUST run **after all writes are applied**, but **before commit finishes**.
* If refresh fails: the entire transaction fails and rolls back.

### R3 — Dependency-correct order

If we have:

```text
tv_company → tv_user → tv_post → tv_feed
```

then for a transaction that touches all of these:

1. `tv_company` must be refreshed before
2. `tv_user` before
3. `tv_post` before
4. `tv_feed`

### R4 — Propagation is part of the same coalesced algorithm

When refreshing `tv_post(pk_post=123)`:

* We may discover parent rows, e.g. `tv_user(pk_user=7)`.
* That parent refresh must also go through the **same transaction-level queue**, so it is also deduped.

### R5 — No extra read-round trips from FraiseQL

FraiseQL just:

* writes to `tb_*`
* then reads from `tv_*`

TVIEW lives entirely in DB.

---

# 3. High-Level Design

### 3.1 Transaction-Local Queue

We maintain a **per-transaction in-memory queue** (HashSet) of refresh requests:

```text
(entity_name: String, pk: i64)
```

This is:

* Populated by row-level triggers on `tb_*` (and potentially `tv_*` if needed)
* Flushed exactly once at **transaction commit**

### 3.2 Commit Callback

We register a **transaction callback** (xact callback) via pgrx / PG hooks:

* On `XactEvent::Commit`, we:

  * Snapshot & clear the queue
  * Resolve entities into dependency order
  * For each (entity, pk), perform the **refresh phase**

### 3.3 Refresh Phase

For each `(entity, pk)`:

1. Recompute the row from `v_entity` (the view).
2. Patch the corresponding `tv_entity` row via `jsonb_ivm_patch`.
3. Determine parents (e.g. from FK columns) and enqueue them into the same transaction queue (if not already processed).

Processing continues until the queue is empty (closure of the graph).

---

# 4. Data Structures

### 4.1 `RefreshKey`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RefreshKey {
    pub entity: String, // "post", "user", "company", etc.
    pub pk: i64,        // pk_<entity> value
}
```

### 4.2 Queue State

Transaction-local state:

```rust
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use pgrx::prelude::*;

thread_local! {
    static TX_REFRESH_QUEUE: RefCell<HashSet<RefreshKey>> = RefCell::new(HashSet::new());
    static TX_REFRESH_SCHEDULED: RefCell<bool> = RefCell::new(false);
}
```

We:

* Use `HashSet<RefreshKey>` for dedup.
* Track whether we’ve already registered a commit callback (`TX_REFRESH_SCHEDULED`).

### 4.3 Dependency Graph

Precomputed dependency DAG between entities:

```rust
/// tv_company -> tv_user -> tv_post -> tv_feed
#[derive(Debug, Clone)]
pub struct EntityDepGraph {
    /// entity_name -> vec of parent entity_names (parents depend on child)
    /// Example: "post" -> ["feed"], "user" -> ["post"]
    pub parents: HashMap<String, Vec<String>>,
    /// entity_name -> vec of child entity_names
    pub children: HashMap<String, Vec<String>>,
    /// topological order of entities (lowest dependency first)
    pub topo_order: Vec<String>,
}
```

This can be loaded once at extension startup or lazily cached (details up to you).

---

# 5. Trigger Behavior (Enqueue Only)

### 5.1 Trigger semantics

* Called for every `INSERT`, `UPDATE`, `DELETE` on any underlying `tb_*`.
* Must **NOT** recompute TVIEWs directly.
* Only identifies which `(entity, pk)` should be refreshed **eventually** and enqueues them.

### 5.2 Trigger pseudo-code

```rust
use pgrx::prelude::*;
use crate::tx_queue::enqueue_refresh_from_trigger;

#[pg_trigger]
pub fn tview_trigger(trigger: &PgTrigger) -> Result<
    Option<pgrx::heap_tuple::PgHeapTuple<'_, pgrx::pgbox::AllocatedByRust>>,
    spi::Error,
> {
    let rel = trigger.relation()?;
    let source_oid = rel.oid();

    // Determine which entity/entities this table affects.
    // Often one table maps to one entity, but read-models can be synthetic.
    // We'll assume we have metadata: "tb_post" -> "post".
    let entity = crate::catalog::entity_for_table(source_oid)?;

    // Extract PK of the changed row.
    let pk = crate::util::extract_pk(trigger, &entity)?;

    // Enqueue the refresh request.
    enqueue_refresh_from_trigger(&entity, pk)?;

    // Standard AFTER trigger: return NEW where present, else OLD.
    Ok(trigger.new_or_old())
}
```

---

# 6. Transaction Queue Logic

## 6.1 Enqueue Function

```rust
// tx_queue.rs
use super::RefreshKey;
use std::collections::HashSet;
use pgrx::prelude::*;

pub fn enqueue_refresh_from_trigger(entity: &str, pk: i64) -> spi::Result<()> {
    let key = RefreshKey { entity: entity.to_string(), pk };

    TX_REFRESH_QUEUE.with(|cell| {
        let mut set = cell.borrow_mut();
        set.insert(key);
    });

    // Ensure commit callback is registered once per transaction
    TX_REFRESH_SCHEDULED.with(|flag_cell| {
        let mut flag = flag_cell.borrow_mut();
        if !*flag {
            register_commit_callback()?;
            *flag = true;
        }
        Ok(())
    })
}

/// Register a transaction callback that will be invoked at COMMIT.
/// Exact API depends on pgrx / PG version; this is pseudocode.
fn register_commit_callback() -> spi::Result<()> {
    // Pseudo-API — you will use pgrx internals or a raw PG hook:
    // pgrx::register_xact_callback(XactEvent::Commit, tx_commit_handler);

    // For the stub, we assume something like:
    crate::xact::register_commit_callback(tx_commit_handler);
    Ok(())
}
```

## 6.2 Commit Handler

```rust
fn tx_commit_handler() -> spi::Result<()> {
    // Take the queue snapshot and clear it
    let mut queue_snapshot: HashSet<RefreshKey> = HashSet::new();

    TX_REFRESH_QUEUE.with(|cell| {
        // swap out the set
        let mut set = cell.borrow_mut();
        queue_snapshot = std::mem::take(&mut *set);
    });

    TX_REFRESH_SCHEDULED.with(|flag_cell| {
        *flag_cell.borrow_mut() = false;
    });

    if queue_snapshot.is_empty() {
        return Ok(());
    }

    // Convert to VecDeque for iterative processing
    let mut worklist: VecDeque<RefreshKey> = queue_snapshot.into_iter().collect();
    let mut processed: HashSet<RefreshKey> = HashSet::new();

    // Optionally, you can sort or bucket by entity to respect topo order.
    // For now, we’ll process iteratively and use DAG logic during propagation.

    // Main refresh loop
    while let Some(key) = worklist.pop_front() {
        if processed.contains(&key) {
            continue;
        }

        // Refresh this (entity, pk)
        crate::refresh::refresh_entity_pk(&key)?;

        processed.insert(key.clone());

        // Propagation: may return parents to enqueue
        let parents = crate::propagate::parents_for(&key)?;

        for parent_key in parents {
            if !processed.contains(&parent_key) {
                worklist.push_back(parent_key);
            }
        }
    }

    Ok(())
}
```

This loop:

* Ensures each `(entity, pk)` is refreshed at most once.
* Uses propagation step to populate further refreshes (parents).
* Terminates because dependency graph is acyclic and finite.

If you prefer topological ordering at the **entity level**, you can:

* Bucket refresh keys by `entity`
* Process entities according to `EntityDepGraph.topo_order`
* But the iterative approach plus a DAG ensures correctness as well.

---

# 7. Refresh Logic

## 7.1 High-Level Steps

For a given `(entity, pk)`:

1. Find `v_entity` and `tv_entity` OIDs from metadata.
2. Run `SELECT * FROM v_entity WHERE pk_<entity> = $1`.
3. If row exists:

   * Extract `data` and any updated FK / UUID columns.
   * Patch `tv_entity` row using `jsonb_ivm_patch`.
4. If row does not exist (e.g. deletion):

   * Delete from `tv_entity`.

## 7.2 Pseudo-code

```rust
use pgrx::prelude::*;
use crate::catalog::TviewMeta;

pub fn refresh_entity_pk(key: &crate::RefreshKey) -> spi::Result<()> {
    let meta = TviewMeta::load_for_entity(&key.entity)?
        .ok_or_else(|| spi::Error::User(format!("No TVIEW meta for entity {}", key.entity)))?;

    let view_name = meta.view_name;      // "v_post"
    let table_name = meta.table_name;    // "tv_post"
    let pk_col = format!("pk_{}", key.entity); // "pk_post"

    // 1. Recompute from view
    let select_sql = format!(
        "SELECT * FROM {} WHERE {} = $1",
        view_name, pk_col
    );

    Spi::connect(|client| {
        let rows = client.select(
            &select_sql,
            None,
            Some(vec![(
                PgOid::BuiltIn(PgBuiltInOids::INT8OID),
                key.pk.into(),
            )]),
        )?;

        if rows.len() == 0 {
            // Row deleted in view → delete tv row
            let delete_sql = format!(
                "DELETE FROM {} WHERE {} = $1",
                table_name, pk_col
            );
            client.update(
                &delete_sql,
                None,
                Some(vec![(
                    PgOid::BuiltIn(PgBuiltInOids::INT8OID),
                    key.pk.into(),
                )]),
            )?;
            return Ok(());
        }

        let row = rows.get(0)?;

        // Extract JSONB data
        let new_data: JsonB = row["data"].value().unwrap();

        // Extract FK columns & UUID FK columns as needed
        let fk_updates = crate::infer::extract_fk_updates(&row, &meta)?;
        let uuid_fk_updates = crate::infer::extract_uuid_fk_updates(&row, &meta)?;

        // 2. Apply patch using jsonb_ivm_patch
        let mut set_fragments: Vec<String> = Vec::new();
        set_fragments.push("data = jsonb_ivm_patch(data, $1)".to_string());
        set_fragments.push("updated_at = now()".to_string());

        // Add fk/uuid columns to SET clause
        for (i, col) in fk_updates.columns.iter().enumerate() {
            set_fragments.push(format!("{} = ${}", col, i + 2)); // offset by 2 (data + pk)
        }
        let uuid_offset = 2 + fk_updates.columns.len();
        for (j, col) in uuid_fk_updates.columns.iter().enumerate() {
            set_fragments.push(format!("{} = ${}", col, uuid_offset + j));
        }

        let set_clause = set_fragments.join(", ");

        let update_sql = format!(
            "UPDATE {} SET {} WHERE {} = ${}",
            table_name,
            set_clause,
            pk_col,
            uuid_offset + uuid_fk_updates.columns.len() + 1
        );

        // Build bind params
        let mut params: Vec<(PgOid, pgrx::Datum)> = Vec::new();
        params.push((PgOid::BuiltIn(PgBuiltInOids::JSONBOID), new_data.into()));

        for v in &fk_updates.values {
            params.push((PgOid::BuiltIn(PgBuiltInOids::INT8OID), (*v).into()));
        }
        for v in &uuid_fk_updates.values {
            params.push((PgOid::BuiltIn(PgBuiltInOids::UUIDOID), (*v).into()));
        }
        params.push((PgOid::BuiltIn(PgBuiltInOids::INT8OID), key.pk.into()));

        client.update(&update_sql, None, Some(params))?;

        Ok(())
    })
}
```

> **Note:** This is intentionally verbose pseudo-code so the team can see how to wire dynamic sets; you can simplify based on your conventions.

---

# 8. Propagation Logic

## 8.1 Goal

Given a refreshed `(entity, pk)`, find which parent entities depend on it and enqueue those.

Two ways:

* **FK-based**: inspect FK columns in the refreshed view row (e.g. `fk_user`) and map them to parents (e.g. `user`).
* **Graph-based**: use precomputed `EntityDepGraph` (which entity depends on which).

### 8.2 Pseudo-code

```rust
use pgrx::prelude::*;
use crate::{RefreshKey, catalog::TviewMeta};

pub fn parents_for(key: &RefreshKey) -> spi::Result<Vec<RefreshKey>> {
    let mut result = Vec::new();

    // 1. Load metadata for this entity
    let meta = TviewMeta::load_for_entity(&key.entity)?
        .ok_or_else(|| spi::Error::User(format!("No meta for {}", key.entity)))?;

    // 2. From entity dep graph, find parent entities
    let dep_graph = crate::catalog::entity_dep_graph()?;
    let parents = dep_graph.parents.get(&key.entity).cloned().unwrap_or_default();

    // 3. For each parent entity, determine which PK value to enqueue
    // Typically: parent.pk = some fk_* value on this row.
    // We need the refreshed fk_* columns. We could:
    //  - either fetch the row from v_entity again and read FKs
    //  - or reuse previous view row if we keep it around.

    for parent_entity in parents {
        let fk_value = crate::util::lookup_fk_for_parent(&key.entity, &parent_entity, key.pk)?;
        if let Some(parent_pk) = fk_value {
            result.push(RefreshKey {
                entity: parent_entity,
                pk: parent_pk,
            });
        }
    }

    Ok(result)
}
```

This is intentionally a **high-level stub**; the real implementation will depend on how you encode parent relationships in your metadata.

---

# 9. Error Handling & Failure Modes

* If `refresh_entity_pk` fails:

  * The commit callback returns `Err`.
  * The entire transaction aborts, including writes to `tb_*`.
  * TVIEW remains in a consistent previous state.
* If propagation discovers missing metadata:

  * Raise a clear `ERROR` about misconfigured TVIEW.
* If multiple triggers race:

  * They all operate on transaction-local state. No cross-transaction interference.

---

# 10. Summary for the Coding Team

**Conceptual behavior:**

* Triggers only **enqueue** refreshes.
* Coalescing happens in a **transaction-local HashSet**.
* Actual refresh work happens **once per (entity,pk) at COMMIT**.
* Refresh uses `v_entity` to recompute and `jsonb_ivm_patch` to apply.
* Propagation is driven by FK and dependency graph, and uses the same queue (so it’s coalesced too).

**Key pieces to implement:**

1. `RefreshKey` + transaction-local queue (`TX_REFRESH_QUEUE`, `TX_REFRESH_SCHEDULED`)
2. Trigger → `enqueue_refresh_from_trigger`
3. Commit callback → `tx_commit_handler`
4. `refresh_entity_pk` (recompute + jsonb_ivm_patch + delete-if-missing)
5. `parents_for` + entity dependency graph
6. Metadata layer (`TviewMeta`) to map entities → v_*/tv_* names, PK/FK columns

If you want, next we can:

* Zoom in on **how to compute the entity dependency graph from `pg_depend`**, with concrete SQL + Rust.
* Or design the `pg_tview_meta` schema in detail.


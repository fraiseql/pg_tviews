# Phase 6D: Entity Dependency Graph

**Status:** BLOCKED (requires Phase 6A, 6B, 6C complete)
**Prerequisites:** Phase 6C Commit Processing ✅
**Estimated Time:** 1-2 days
**TDD Phase:** RED → GREEN → REFACTOR

---

## Objective

Implement dependency-correct refresh ordering:

1. Build entity dependency graph from `pg_tview_meta`
2. Implement topological sorting of refresh queue
3. Integrate propagation with queue (coalesce parent refreshes)
4. Cache dependency graph for performance

---

## Context

### Current State (After Phase 6C)

Refreshes happen in queue insertion order (NOT dependency order):

```rust
BEGIN;
UPDATE tb_post SET title = 'New' WHERE pk_post = 1;  -- depends on tv_user
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1; -- depends on tv_company

// Queue: [("post", 1), ("user", 1)]
// Refresh order: post, then user ❌ WRONG
// Problem: tv_post refresh reads stale tv_user data
```

### Target State (Phase 6D)

Refreshes happen in dependency order:

```rust
// Dependency graph: tv_company → tv_user → tv_post → tv_feed
// Queue: {("post", 1), ("user", 1), ("company", 1)}
// Sorted by dependencies: [("company", 1), ("user", 1), ("post", 1)] ✅
// Refresh order: company, then user, then post (correct)
```

---

## Entity Dependency Graph

From `PRD_multiupdate.md`:

```rust
/// tv_company → tv_user → tv_post → tv_feed
#[derive(Debug, Clone)]
pub struct EntityDepGraph {
    /// entity_name -> vec of parent entity_names (parents depend on child)
    /// Example: "post" -> ["feed"], "user" -> ["post"]
    pub parents: HashMap<String, Vec<String>>,

    /// entity_name -> vec of child entity_names
    pub children: HashMap<String, Vec<String>>,

    /// topological order of entities (lowest dependency first)
    /// Example: ["company", "user", "post", "feed"]
    pub topo_order: Vec<String>,
}
```

**Construction**:
1. Query `pg_tview_meta` for all entities and their FK columns
2. Build parent/child relationships (e.g., `tv_post.fk_user` → `tv_user` is parent)
3. Topological sort to get refresh order
4. Cache in memory for performance

---

## Files to Create

### 1. `src/queue/graph.rs` (NEW)

Entity dependency graph:

```rust
use std::collections::{HashMap, HashSet, VecDeque};
use pgrx::prelude::*;
use crate::TViewResult;

/// Entity dependency graph for refresh ordering
///
/// Example:
/// - tv_company (no dependencies)
/// - tv_user (depends on tv_company via fk_company)
/// - tv_post (depends on tv_user via fk_user)
/// - tv_feed (depends on tv_post via fk_post)
///
/// Topological order: ["company", "user", "post", "feed"]
#[derive(Debug, Clone)]
pub struct EntityDepGraph {
    /// Parent relationships: entity -> list of entities that depend on it
    /// Example: "user" -> ["post", "feed"]
    pub parents: HashMap<String, Vec<String>>,

    /// Child relationships: entity -> list of entities it depends on
    /// Example: "post" -> ["user"]
    pub children: HashMap<String, Vec<String>>,

    /// Topological order (refresh from low to high dependency)
    /// Example: ["company", "user", "post", "feed"]
    pub topo_order: Vec<String>,
}

impl EntityDepGraph {
    /// Build dependency graph from pg_tview_meta
    pub fn load() -> TViewResult<Self> {
        // Query pg_tview_meta for all entities and their FK columns
        let query = "SELECT entity, fk_columns FROM pg_tview_meta";

        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut all_entities: HashSet<String> = HashSet::new();

        Spi::connect(|client| {
            let rows = client.select(query, None, None)?;

            for row in rows {
                let entity: String = row["entity"].value().unwrap().unwrap();
                let fk_columns: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);

                all_entities.insert(entity.clone());

                if let Some(fk_cols) = fk_columns {
                    for fk_col in fk_cols {
                        // FK column format: "fk_<entity>"
                        // Example: "fk_user" -> "user"
                        if let Some(parent_entity) = fk_col.strip_prefix("fk_") {
                            // Register parent relationship
                            parents.entry(parent_entity.to_string())
                                .or_insert_with(Vec::new)
                                .push(entity.clone());

                            // Register child relationship
                            children.entry(entity.clone())
                                .or_insert_with(Vec::new)
                                .push(parent_entity.to_string());
                        }
                    }
                }
            }

            Ok::<_, spi::SpiError>(())
        })?;

        // Compute topological order
        let topo_order = topological_sort(&all_entities, &children)?;

        Ok(Self {
            parents,
            children,
            topo_order,
        })
    }

    /// Sort refresh keys by dependency order
    ///
    /// Keys are grouped by entity, then sorted by topo_order.
    /// Within each entity group, PK order is preserved.
    pub fn sort_keys(&self, keys: Vec<super::key::RefreshKey>) -> Vec<super::key::RefreshKey> {
        // Group by entity
        let mut groups: HashMap<String, Vec<i64>> = HashMap::new();
        for key in keys {
            groups.entry(key.entity.clone())
                .or_insert_with(Vec::new)
                .push(key.pk);
        }

        // Sort entities by topo_order
        let mut sorted_keys = Vec::new();
        for entity in &self.topo_order {
            if let Some(pks) = groups.get(entity) {
                for pk in pks {
                    sorted_keys.push(super::key::RefreshKey {
                        entity: entity.clone(),
                        pk: *pk,
                    });
                }
            }
        }

        sorted_keys
    }
}

/// Topological sort using Kahn's algorithm
fn topological_sort(
    entities: &HashSet<String>,
    children: &HashMap<String, Vec<String>>,
) -> TViewResult<Vec<String>> {
    // Calculate in-degree for each entity
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    for entity in entities {
        in_degree.insert(entity.clone(), 0);
    }

    for deps in children.values() {
        for dep in deps {
            *in_degree.entry(dep.clone()).or_insert(0) += 1;
        }
    }

    // Start with entities that have no dependencies
    let mut queue: VecDeque<String> = VecDeque::new();
    for (entity, &degree) in &in_degree {
        if degree == 0 {
            queue.push_back(entity.clone());
        }
    }

    let mut result = Vec::new();

    while let Some(entity) = queue.pop_front() {
        result.push(entity.clone());

        // Find entities that depend on this one
        if let Some(parents) = children.get(&entity) {
            for parent in parents {
                if let Some(degree) = in_degree.get_mut(parent) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(parent.clone());
                    }
                }
            }
        }
    }

    // Check for cycles
    if result.len() != entities.len() {
        return Err(crate::TViewError::DependencyCycle {
            entities: entities.iter().cloned().collect(),
        });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topological_sort() {
        // Entity graph:
        // company (no deps)
        // user -> company
        // post -> user
        // feed -> post

        let entities: HashSet<String> = ["company", "user", "post", "feed"]
            .iter().map(|s| s.to_string()).collect();

        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        children.insert("user".to_string(), vec!["company".to_string()]);
        children.insert("post".to_string(), vec!["user".to_string()]);
        children.insert("feed".to_string(), vec!["post".to_string()]);

        let topo = topological_sort(&entities, &children).unwrap();

        // Valid topological orders:
        // ["company", "user", "post", "feed"]
        // Check that company comes before user, user before post, etc.
        let company_idx = topo.iter().position(|e| e == "company").unwrap();
        let user_idx = topo.iter().position(|e| e == "user").unwrap();
        let post_idx = topo.iter().position(|e| e == "post").unwrap();
        let feed_idx = topo.iter().position(|e| e == "feed").unwrap();

        assert!(company_idx < user_idx);
        assert!(user_idx < post_idx);
        assert!(post_idx < feed_idx);
    }
}
```

### 2. `src/error.rs` (MODIFY)

Add new error variants:

```rust
#[derive(Debug, Clone)]
pub enum TViewError {
    // ... existing variants ...

    /// Dependency cycle detected in entity graph
    DependencyCycle {
        entities: Vec<String>,
    },

    /// Propagation exceeded maximum depth (possible infinite loop)
    PropagationDepthExceeded {
        max_depth: usize,
        processed: usize,
    },
}
```

**Display implementation:**

```rust
impl std::fmt::Display for TViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // ... existing variants ...

            TViewError::DependencyCycle { entities } => {
                write!(f, "Dependency cycle detected in entity graph: {}", entities.join(" -> "))
            }

            TViewError::PropagationDepthExceeded { max_depth, processed } => {
                write!(
                    f,
                    "Propagation exceeded maximum depth of {} iterations ({} entities processed). \
                     Possible infinite loop or extremely deep dependency chain.",
                    max_depth, processed
                )
            }
        }
    }
}
```

---

## Files to Modify

### 1. `src/queue/xact.rs`

**Replace entire `handle_pre_commit()` function** with proper propagation integration:

```rust
use super::graph::EntityDepGraph;
use std::collections::HashSet;

/// Handle PRE_COMMIT event: flush the queue and refresh TVIEWs
///
/// This implementation correctly handles propagation by using a local queue
/// for discovered parent refreshes. The workflow:
///
/// 1. Take snapshot of triggered refreshes (from triggers)
/// 2. Process in dependency order (children before parents)
/// 3. Discover parent refreshes during processing
/// 4. Add parents to local pending queue
/// 5. Repeat until no more refreshes discovered (fixpoint)
///
/// # Correctness
///
/// - Each (entity, pk) processed exactly once (tracked in `processed` set)
/// - Dependency order respected (topological sort per iteration)
/// - Propagation coalesced (parents discovered during refresh added to queue)
/// - Transaction-safe (fail-fast aborts transaction on first error)
fn handle_pre_commit() -> TViewResult<()> {
    // Take initial snapshot from triggers
    let mut pending = take_queue_snapshot();

    if pending.is_empty() {
        return Ok(());
    }

    info!("TVIEW: Flushing {} initial refresh requests at commit", pending.len());

    // Load dependency graph once (cached)
    let graph = EntityDepGraph::load_cached()?;

    // Track processed keys to avoid duplicates
    let mut processed: HashSet<RefreshKey> = HashSet::new();

    // Process queue until empty (handles propagation)
    let mut iteration = 1;
    while !pending.is_empty() {
        // Sort this batch by dependency order
        let sorted_keys = graph.sort_keys(pending.drain().collect());

        info!("TVIEW: Processing iteration {}: {} refreshes", iteration, sorted_keys.len());

        // Process each key in dependency order
        for key in sorted_keys {
            // Skip if already processed (deduplication)
            if !processed.insert(key.clone()) {
                continue;
            }

            // Refresh this entity and discover parents
            // FAIL-FAST: Propagate error immediately to abort transaction
            let parents = refresh_and_get_parents(&key)?;

            // Add discovered parents to pending queue
            for parent_key in parents {
                if !processed.contains(&parent_key) {
                    pending.insert(parent_key);
                }
            }
        }

        iteration += 1;

        // Safety check: prevent infinite loops
        if iteration > 100 {
            return Err(TViewError::PropagationDepthExceeded {
                max_depth: 100,
                processed: processed.len(),
            });
        }
    }

    info!("TVIEW: Completed {} refresh operations in {} iterations", processed.len(), iteration - 1);

    Ok(())
}

/// Refresh a single entity+pk and return discovered parent keys (without refreshing them)
///
/// This function:
/// 1. Refreshes the given (entity, pk) using existing refresh logic
/// 2. Discovers parent entities that depend on this one
/// 3. Returns parent keys WITHOUT refreshing them (defer to queue)
///
/// # Difference from Phase 1-5
///
/// Phase 1-5: `propagate_from_row()` called `refresh_pk()` recursively ❌
/// Phase 6D: Return parent keys for queue processing ✅
fn refresh_and_get_parents(key: &RefreshKey) -> TViewResult<Vec<RefreshKey>> {
    // Load metadata
    use crate::catalog::TviewMeta;
    let meta = TviewMeta::load_by_entity(&key.entity)?
        .ok_or_else(|| TViewError::MetadataNotFound {
            entity: key.entity.clone(),
        })?;

    // Refresh this entity (existing logic)
    crate::refresh::refresh_pk(meta.view_oid, key.pk)?;

    // Find parent entities (NEW: returns keys instead of refreshing)
    let parent_keys = crate::propagate::find_parents_for(key)?;

    Ok(parent_keys)
}
```

### 2. `src/queue/mod.rs`

Add graph module:

```rust
mod key;
mod state;
mod ops;
mod xact;
mod graph;  // NEW

pub use key::RefreshKey;
pub use ops::{enqueue_refresh, take_queue_snapshot, clear_queue, register_commit_callback_once};
pub use graph::EntityDepGraph;  // Export for testing
```

---

## Implementation Steps

### Step 1: Implement EntityDepGraph (RED → GREEN)

1. **RED**: Write tests in `src/queue/graph.rs`
   - `test_topological_sort()`
   - `test_sort_keys()`

2. **GREEN**: Implement:
   - `EntityDepGraph::load()` - Query pg_tview_meta
   - `EntityDepGraph::sort_keys()` - Group and sort
   - `topological_sort()` - Kahn's algorithm

3. **Test**: `cargo test --lib queue::graph`

### Step 2: Integrate with Commit Handler (GREEN)

1. Modify `handle_pre_commit()` in `src/queue/xact.rs`
2. Load graph and sort keys before processing
3. Test with manual SQL:
   ```sql
   BEGIN;
   UPDATE tb_post SET title = 'New' WHERE pk_post = 1;
   UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
   UPDATE tb_company SET name = 'Acme' WHERE pk_company = 1;
   COMMIT;
   -- Check logs: should refresh in order (company, user, post)
   ```

### Step 3: Add Graph Caching (REFACTOR)

Optimize by caching the graph (optional performance enhancement):

```rust
use std::sync::{Mutex, Arc};
use once_cell::sync::Lazy;

static ENTITY_GRAPH_CACHE: Lazy<Mutex<Option<EntityDepGraph>>> = Lazy::new(|| {
    Mutex::new(None)
});

impl EntityDepGraph {
    pub fn load_cached() -> TViewResult<Self> {
        let mut cache = ENTITY_GRAPH_CACHE.lock().unwrap();

        if let Some(graph) = cache.as_ref() {
            return Ok(graph.clone());
        }

        let graph = Self::load()?;
        *cache = Some(graph.clone());
        Ok(graph)
    }

    pub fn invalidate_cache() {
        let mut cache = ENTITY_GRAPH_CACHE.lock().unwrap();
        *cache = None;
    }
}
```

Call `invalidate_cache()` when:
- `CREATE TVIEW` adds a new entity
- `DROP TVIEW` removes an entity
- Metadata is updated

### Step 4: Propagation Integration (REFACTOR)

Refactor `src/propagate.rs` to separate parent discovery from refresh execution:

**Create new function: `find_parents_for()`**

```rust
// In src/propagate.rs

/// Find parent keys that depend on the given entity+pk (without refreshing them)
///
/// This is the Phase 6D version of propagation that returns keys instead of
/// performing immediate recursive refreshes.
///
/// # Example
///
/// ```
/// let key = RefreshKey { entity: "user".into(), pk: 1 };
/// let parents = find_parents_for(&key)?;
/// // Returns: [
/// //   RefreshKey { entity: "post", pk: 10 },
/// //   RefreshKey { entity: "post", pk: 20 },
/// //   RefreshKey { entity: "comment", pk: 5 },
/// // ]
/// // These are all the tv_post and tv_comment rows where fk_user = 1
/// ```
pub fn find_parents_for(key: &RefreshKey) -> TViewResult<Vec<RefreshKey>> {
    use crate::queue::RefreshKey;

    // Find all parent entities that depend on this entity
    let parent_entities = find_parent_entities(&key.entity)?;

    if parent_entities.is_empty() {
        return Ok(Vec::new());
    }

    let mut parent_keys = Vec::new();

    // For each parent entity, find affected rows
    for parent_entity in parent_entities {
        let affected_pks = find_affected_pks(&parent_entity, &key.entity, key.pk)?;

        // Convert to RefreshKeys
        for pk in affected_pks {
            parent_keys.push(RefreshKey {
                entity: parent_entity.clone(),
                pk,
            });
        }
    }

    Ok(parent_keys)
}

/// Legacy propagation function (keep for Phase 1-5 compatibility)
///
/// **DEPRECATED in Phase 6**: This function performs immediate recursive refreshes,
/// which bypasses the transaction queue. Use `find_parents_for()` instead.
///
/// This function is kept for:
/// - Backward compatibility with Phase 1-5 tests
/// - Potential fallback mode if Phase 6 is disabled
pub fn propagate_from_row(row: &ViewRow) -> spi::Result<()> {
    // Existing Phase 1-5 implementation (unchanged)
    // ...
}
```

**Refactor existing helpers to be reusable:**

```rust
// These functions are already implemented (src/propagate.rs:57-117)
// Keep them as-is, they're used by both find_parents_for() and propagate_from_row()

fn find_parent_entities(child_entity: &str) -> spi::Result<Vec<String>> {
    // Existing implementation (lines 57-85)
}

fn find_affected_pks(parent_entity: &str, child_entity: &str, child_pk: i64) -> spi::Result<Vec<i64>> {
    // Existing implementation (lines 87-117)
}
```

---

## Verification Commands

### Compilation Check
```bash
cargo clippy --release -- -D warnings
```

### Unit Tests
```bash
cargo test --lib queue::graph
```

### Integration Test

```sql
-- Setup complex dependency chain
CREATE TABLE tb_company (pk_company INT PRIMARY KEY, name TEXT);
CREATE TABLE tb_user (pk_user INT PRIMARY KEY, name TEXT, fk_company INT);
CREATE TABLE tb_post (pk_post INT PRIMARY KEY, title TEXT, fk_user INT);

INSERT INTO tb_company VALUES (1, 'Acme Corp');
INSERT INTO tb_user VALUES (1, 'Alice', 1);
INSERT INTO tb_post VALUES (1, 'Hello World', 1);

-- Create TVIEWs (assuming pg_tviews_create exists)
SELECT pg_tviews_create('company', 'SELECT pk_company, jsonb_build_object(''name'', name) AS data FROM tb_company');
SELECT pg_tviews_create('user', 'SELECT pk_user, fk_company, jsonb_build_object(''name'', name) AS data FROM tb_user');
SELECT pg_tviews_create('post', 'SELECT pk_post, fk_user, jsonb_build_object(''title'', title) AS data FROM tb_post');

-- Test: Update in reverse dependency order
BEGIN;
UPDATE tb_post SET title = 'Updated Post' WHERE pk_post = 1;
UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1;
UPDATE tb_company SET name = 'Acme Updated' WHERE pk_company = 1;
COMMIT;

-- Check logs: should show refresh order (company → user → post)
-- Expected log:
-- INFO: TVIEW: Flushing 3 refresh requests at commit
-- INFO: Refreshing tv_company[1]
-- INFO: Refreshing tv_user[1]
-- INFO: Refreshing tv_post[1]
```

---

## Acceptance Criteria

- ✅ Dependency graph loads from pg_tview_meta
- ✅ Topological sort produces correct order
- ✅ Commit handler processes refreshes in dependency order
- ✅ No cycles detected (or error raised if cycle exists)
- ✅ Graph caching improves performance
- ✅ Propagation integrated with queue (parents enqueued, not refreshed immediately)
- ✅ All unit tests pass
- ✅ Integration test demonstrates correct ordering
- ✅ Clippy strict compliance (0 warnings)

---

## DO NOT

- ❌ Optimize for very large graphs (100+ entities) - YAGNI for now
- ❌ Implement parallel refresh (single-threaded is fine)
- ❌ Add graph visualization tools (nice-to-have, not required)

---

## Completion Checklist

After Phase 6D is complete, verify all PRD requirements:

- ✅ **R1** (Refresh coalescing): HashSet deduplicates `(entity, pk)` pairs
- ✅ **R2** (End-of-transaction semantics): PRE_COMMIT callback runs before commit
- ✅ **R3** (Dependency-correct order): Topological sort ensures correct order
- ✅ **R4** (Propagation coalescing): Parents enqueued, not refreshed immediately
- ✅ **R5** (No extra round trips): FraiseQL writes to `tb_*`, reads from `tv_*` (no change)

---

## Phase 6 Complete!

After Phase 6D, the transaction-level queue architecture is fully implemented.

### Next Steps

1. **Documentation**: Update CHANGELOG, README, ARCHITECTURE.md
2. **Performance Testing**: Benchmark multi-update workloads
3. **Migration Guide**: Document behavior change for users
4. **Feature Flag**: Consider adding `pg_tviews.refresh_mode` GUC
5. **Production Readiness**: Thorough testing, monitoring, observability

**Congratulations!** Phase 6 implements all 5 PRD requirements (R1-R5).

# Phase 1.4: Refactor Large Functions

**Objective**: Reduce cyclomatic complexity and improve maintainability by refactoring large functions

**Priority**: HIGH
**Estimated Time**: 1-2 days
**Blockers**: Phase 1.2, 1.3 complete

---

## Context

**Current State**: Large, complex functions

**From analysis**:
```
src/refresh/main.rs:      1,117 lines (module total)
src/refresh/array_ops.rs:  663 lines
src/ddl/create.rs:          554 lines
src/hooks.rs:               526 lines
```

**Why This Matters**:
- Functions >100 lines are hard to understand and test
- High cyclomatic complexity increases bug risk
- SQL generation logic is intertwined with business logic
- Violates Single Responsibility Principle

---

## Target Metrics

**Industry Best Practices**:
- **Function length**: <100 lines (ideal: <50)
- **Cyclomatic complexity**: <15 (ideal: <10)
- **Number of arguments**: <5
- **Nesting depth**: <4 levels

---

## Files to Refactor (Priority Order)

### High Priority
1. `src/refresh/main.rs` - Core refresh logic
2. `src/ddl/create.rs` - TVIEW creation
3. `src/hooks.rs` - PostgreSQL hooks

### Medium Priority
4. `src/refresh/array_ops.rs` - Array operations
5. `src/catalog.rs` - Catalog management
6. `src/dependency/graph.rs` - Dependency resolution

---

## Implementation Steps

### Step 1: Measure Current Complexity

**Install tools**:
```bash
cargo install cargo-geiger  # Unsafe code analysis
cargo install tokei         # Line count statistics
```

**Generate baseline metrics**:
```bash
# Function complexity
cargo clippy -- -W clippy::cognitive_complexity 2>&1 | tee /tmp/complexity-before.txt

# Line counts per function
tokei src/ --sort lines

# Cyclomatic complexity (manual inspection)
rg "^(pub )?fn " src/refresh/main.rs | wc -l  # Count functions
```

### Step 2: Refactor `src/refresh/main.rs`

**Target**: `refresh_tview_bulk()` and similar large functions

**Current structure** (hypothetical):
```rust
// Monolithic 200-line function
pub fn refresh_tview_bulk(entities: &[RefreshKey]) -> TViewResult<()> {
    // 1. Validation (30 lines)
    // 2. Dependency resolution (40 lines)
    // 3. SQL generation (60 lines)
    // 4. Execution (40 lines)
    // 5. Error handling (30 lines)
}
```

**Refactored structure**:
```rust
// Main orchestration (30 lines)
pub fn refresh_tview_bulk(entities: &[RefreshKey]) -> TViewResult<()> {
    let validated = validate_refresh_keys(entities)?;
    let sorted = resolve_refresh_order(&validated)?;
    let sql = build_bulk_refresh_sql(&sorted)?;
    execute_bulk_refresh(&sql)?;
    Ok(())
}

// Each sub-function is focused and testable
fn validate_refresh_keys(keys: &[RefreshKey]) -> TViewResult<Vec<ValidatedKey>> {
    // 20 lines
}

fn resolve_refresh_order(keys: &[ValidatedKey]) -> TViewResult<Vec<ValidatedKey>> {
    // 30 lines
}

fn build_bulk_refresh_sql(keys: &[ValidatedKey]) -> TViewResult<String> {
    // 40 lines - or further decomposed
}

fn execute_bulk_refresh(sql: &str) -> TViewResult<()> {
    // 25 lines
}
```

**Benefits**:
- Each function is <50 lines
- Single Responsibility Principle
- Easier to test in isolation
- Clear control flow

### Step 3: Extract SQL Builders

**Create new module**: `src/refresh/sql_builder.rs`

**Before** (in refresh/main.rs):
```rust
pub fn refresh_tview(...) -> TViewResult<()> {
    // 80 lines of SQL string formatting
    let sql = format!(r#"
        WITH new_data AS (
            SELECT {pk_column} as pk, {data_expr} as data
            FROM {backing_view}
            WHERE {pk_column} = ANY($1)
        )
        UPDATE {tview_table} t
        SET data = CASE
            WHEN EXISTS(SELECT 1 FROM pg_tviews_jsonb_ivm_functions())
            THEN pg_tviews_jsonb_patch(t.data, n.data)
            ELSE n.data
        END
        FROM new_data n
        WHERE t.{pk_column} = n.pk
    "#, pk_column=..., data_expr=..., ...);

    // Execute...
}
```

**After**:
```rust
// src/refresh/main.rs
pub fn refresh_tview(...) -> TViewResult<()> {
    let sql = RefreshSqlBuilder::new(metadata)
        .with_keys(&keys)
        .with_jsonb_optimization(check_jsonb_ivm_available())
        .build()?;

    execute_refresh_sql(&sql)
}

// src/refresh/sql_builder.rs
pub struct RefreshSqlBuilder<'a> {
    metadata: &'a TViewMetadata,
    keys: Option<&'a [i64]>,
    use_jsonb_ivm: bool,
}

impl<'a> RefreshSqlBuilder<'a> {
    pub fn new(metadata: &'a TViewMetadata) -> Self { ... }

    pub fn with_keys(mut self, keys: &'a [i64]) -> Self {
        self.keys = Some(keys);
        self
    }

    pub fn with_jsonb_optimization(mut self, enabled: bool) -> Self {
        self.use_jsonb_ivm = enabled;
        self
    }

    pub fn build(&self) -> TViewResult<String> {
        let mut sql = String::with_capacity(512);

        self.build_cte(&mut sql)?;
        self.build_update(&mut sql)?;
        self.build_where_clause(&mut sql)?;

        Ok(sql)
    }

    fn build_cte(&self, buf: &mut String) -> TViewResult<()> {
        // 20 lines - focused on CTE generation
    }

    fn build_update(&self, buf: &mut String) -> TViewResult<()> {
        // 25 lines - focused on UPDATE clause
    }

    fn build_where_clause(&self, buf: &mut String) -> TViewResult<()> {
        // 15 lines - focused on WHERE clause
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_refresh_sql_with_jsonb_ivm() {
        let metadata = create_test_metadata();
        let sql = RefreshSqlBuilder::new(&metadata)
            .with_jsonb_optimization(true)
            .build()
            .unwrap();

        assert!(sql.contains("pg_tviews_jsonb_patch"));
    }

    #[test]
    fn test_refresh_sql_without_jsonb_ivm() {
        let metadata = create_test_metadata();
        let sql = RefreshSqlBuilder::new(&metadata)
            .with_jsonb_optimization(false)
            .build()
            .unwrap();

        assert!(!sql.contains("pg_tviews_jsonb_patch"));
        assert!(sql.contains("UPDATE"));
    }
}
```

### Step 4: Refactor `src/ddl/create.rs`

**Extract validation logic**:

**Create**: `src/ddl/validation.rs`

```rust
pub struct TViewDefinitionValidator {
    definition: String,
}

impl TViewDefinitionValidator {
    pub fn new(definition: String) -> Self {
        Self { definition }
    }

    pub fn validate(&self) -> TViewResult<ValidatedDefinition> {
        self.validate_syntax()?;
        self.validate_required_columns()?;
        self.validate_backing_view()?;

        Ok(ValidatedDefinition {
            sql: self.definition.clone(),
            pk_column: self.extract_pk_column()?,
            data_column: self.extract_data_column()?,
        })
    }

    fn validate_syntax(&self) -> TViewResult<()> { /* 15 lines */ }
    fn validate_required_columns(&self) -> TViewResult<()> { /* 20 lines */ }
    fn validate_backing_view(&self) -> TViewResult<()> { /* 25 lines */ }
    fn extract_pk_column(&self) -> TViewResult<String> { /* 15 lines */ }
    fn extract_data_column(&self) -> TViewResult<String> { /* 15 lines */ }
}
```

**Usage in create.rs**:
```rust
pub fn create_tview(definition: &str) -> TViewResult<()> {
    // Was 100+ lines, now 30
    let validated = TViewDefinitionValidator::new(definition.to_string())
        .validate()?;

    let metadata = build_metadata(&validated)?;
    store_metadata(&metadata)?;
    create_triggers(&metadata)?;

    Ok(())
}
```

### Step 5: Add Unit Tests for Extracted Functions

**For each extracted function, add tests**:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_refresh_keys_empty() {
        let result = validate_refresh_keys(&[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }

    #[test]
    fn test_validate_refresh_keys_duplicates() {
        let keys = vec![
            RefreshKey { entity: "user".into(), pk: 1 },
            RefreshKey { entity: "user".into(), pk: 1 },  // Duplicate
        ];
        let result = validate_refresh_keys(&keys);
        assert!(result.is_ok());
        // Should deduplicate
        assert_eq!(result.unwrap().len(), 1);
    }

    #[test]
    fn test_build_refresh_sql_single_key() {
        let metadata = test_metadata();
        let keys = vec![ValidatedKey { pk: 1 }];
        let sql = build_bulk_refresh_sql(&keys).unwrap();

        assert!(sql.contains("UPDATE tv_"));
        assert!(sql.contains("WHERE"));
    }
}
```

### Step 6: Verify Complexity Reduction

**After refactoring**:
```bash
# Re-run complexity analysis
cargo clippy -- -W clippy::cognitive_complexity 2>&1 | tee /tmp/complexity-after.txt

# Compare
diff -u /tmp/complexity-before.txt /tmp/complexity-after.txt

# Check function counts (should increase as we split)
rg "^(pub )?fn " src/refresh/main.rs | wc -l

# Check average function length (should decrease)
tokei src/refresh/main.rs
```

---

## Verification Commands

```bash
# 1. No function >100 lines
rg -A 100 "^(pub )?fn " src/ --type rust | \
  rg -c "^(pub )?fn " | \
  awk -F: '$2 > 100 { print $1 " has long functions" }'

# 2. All tests pass
cargo test --all

# 3. Integration tests
cargo pgrx test pg17

# 4. No performance regression
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh

# 5. Clippy clean
cargo clippy --all-targets -- -D warnings

# 6. Documentation builds
cargo doc --no-deps
```

---

## Acceptance Criteria

- [x] No function exceeds 100 lines (excluding tests)
- [x] Cyclomatic complexity <15 for all functions
- [x] SQL builders extracted to dedicated module
- [x] Validation logic separated from business logic
- [x] All extracted functions have unit tests
- [x] All integration tests still pass
- [x] No performance regression
- [x] Documentation updated for new modules

---

## DO NOT

- ❌ Over-engineer - keep it simple
- ❌ Extract functions that are only called once
- ❌ Create deep inheritance hierarchies
- ❌ Add unnecessary abstractions
- ❌ Change public API (defer to Phase 4)

---

## Refactoring Patterns

### Extract Function
**When**: Code block >20 lines or has clear responsibility
```rust
// Before
fn big_function() {
    // Block 1: 30 lines
    // Block 2: 30 lines
}

// After
fn big_function() {
    block_1();
    block_2();
}
fn block_1() { /* 30 lines */ }
fn block_2() { /* 30 lines */ }
```

### Extract Builder
**When**: Many configuration parameters
```rust
// Before
fn create(arg1: X, arg2: Y, arg3: Z, arg4: A, arg5: B) { }

// After
struct Builder { arg1: X, arg2: Y, ... }
impl Builder {
    fn new() -> Self { }
    fn with_arg1(self, arg1: X) -> Self { }
    fn build() -> Result { }
}
```

### Extract Validator
**When**: Validation logic >30 lines
```rust
// Before
fn process(input: &str) -> Result {
    // 50 lines of validation
    // 20 lines of processing
}

// After
struct Validator;
impl Validator {
    fn validate(input: &str) -> Result<Validated> { }
}
fn process(input: &str) -> Result {
    let validated = Validator::validate(input)?;
    // 20 lines of processing
}
```

---

## File Organization

**After refactoring**, module structure should be:

```
src/refresh/
├── mod.rs           # Public API, orchestration
├── main.rs          # Core refresh logic (refactored, <300 lines)
├── sql_builder.rs   # SQL generation (new)
├── validation.rs    # Input validation (new)
├── cache.rs         # Caching layer
├── bulk.rs          # Bulk operations
├── batch.rs         # Batch processing
└── array_ops.rs     # Array operations

src/ddl/
├── mod.rs           # Public API
├── create.rs        # TVIEW creation (refactored, <200 lines)
├── convert.rs       # Table conversion
├── drop.rs          # TVIEW deletion
└── validation.rs    # DDL validation (new)
```

---

## Performance Considerations

**Refactoring should NOT impact performance**:
- Function calls are inlined by compiler (`#[inline]`)
- Builder pattern is zero-cost (compile-time)
- Extracted validators run same logic

**Benchmark critical paths**:
```bash
# Profile before
cargo flamegraph --bin pgrx_embed_pg_tviews

# Refactor

# Profile after
cargo flamegraph --bin pgrx_embed_pg_tviews

# Compare flame graphs
```

---

## Next Steps

After completion:
- Commit with message: `refactor(core): Reduce function complexity <100 LOC [PHASE1.4]`
- Update architecture documentation with new module structure
- Proceed to **Phase 2.1: Concurrency Stress Testing**

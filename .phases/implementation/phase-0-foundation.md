# Phase 0: Foundation & Project Setup

**Status:** Planning
**Duration:** 1-2 days
**Complexity:** Low
**Prerequisites:** Rust toolchain, pgrx 0.12.8, PostgreSQL 15+

---

## Objective

Establish the project foundation with proper Rust/pgrx structure, testing infrastructure, and minimal working extension that can be loaded into PostgreSQL.

---

## Success Criteria

- [ ] Extension compiles successfully with pgrx
- [ ] Extension loads into PostgreSQL 15+ without errors
- [ ] Basic test infrastructure works (Rust unit tests + SQL integration tests)
- [ ] CI/CD pipeline validates builds on PostgreSQL 15, 16, 17
- [ ] Development workflow documented (build, test, install)

---

## TDD Approach: RED → GREEN → REFACTOR

### Test 1: Extension Loads Successfully

**RED Phase - Write Failing Test:**

```sql
-- test/sql/00_extension_loading.sql
-- Test: Extension can be created
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Verify extension exists
    SELECT COUNT(*) = 1 AS extension_loaded
    FROM pg_extension
    WHERE extname = 'pg_tviews';

    -- Expected: t (true)
ROLLBACK;
```

**Expected Output (failing):**
```
ERROR: extension "pg_tviews" is not available
```

**GREEN Phase - Minimal Implementation:**

```rust
// src/lib.rs
use pgrx::prelude::*;

::pgrx::pg_module_magic!();

#[pg_extern]
fn pg_tviews_version() -> &'static str {
    "0.1.0-alpha"
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_extension_loads() {
        // Extension loaded successfully if this test runs
        assert!(true);
    }
}
```

```toml
# Cargo.toml
[package]
name = "pg_tviews"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[[bin]]
name = "pgrx_embed_pg_tviews"
path = "./src/bin/pgrx_embed.rs"

[dependencies]
pgrx = "=0.12.8"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

[dev-dependencies]
pgrx-tests = "=0.12.8"

[features]
default = ["pg17"]
pg12 = ["pgrx/pg12", "pgrx-tests/pg12"]
pg13 = ["pgrx/pg13", "pgrx-tests/pg13"]
pg14 = ["pgrx/pg14", "pgrx-tests/pg14"]
pg15 = ["pgrx/pg15", "pgrx-tests/pg15"]
pg16 = ["pgrx/pg16", "pgrx-tests/pg16"]
pg17 = ["pgrx/pg17", "pgrx-tests/pg17"]

[profile.dev]
panic = "unwind"

[profile.release]
panic = "unwind"
opt-level = 3
lto = "fat"
codegen-units = 1
```

```sql
-- pg_tviews.control
comment = 'Incremental JSONB view maintenance for PostgreSQL'
default_version = '0.1.0'
module_pathname = '$libdir/pg_tviews'
relocatable = false
requires = 'jsonb_delta'
```

**Verify GREEN:**
```bash
cargo pgrx install --release
psql -d test_db -c "CREATE EXTENSION pg_tviews;"
psql -d test_db -f test/sql/00_extension_loading.sql
```

**Expected Output:**
```
 extension_loaded
------------------
 t
(1 row)
```

---

### Test 2: Metadata Table Creation

**RED Phase - Write Failing Test:**

```sql
-- test/sql/01_metadata_tables.sql
-- Test: Metadata tables exist after extension creation
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Test 1: pg_tview_meta table exists
    SELECT COUNT(*) = 1 AS meta_table_exists
    FROM information_schema.tables
    WHERE table_schema = 'public'
      AND table_name = 'pg_tview_meta';

    -- Test 2: pg_tview_helpers table exists
    SELECT COUNT(*) = 1 AS helpers_table_exists
    FROM information_schema.tables
    WHERE table_schema = 'public'
      AND table_name = 'pg_tview_helpers';

    -- Test 3: Verify pg_tview_meta schema
    SELECT
        column_name,
        data_type,
        is_nullable
    FROM information_schema.columns
    WHERE table_name = 'pg_tview_meta'
    ORDER BY ordinal_position;

    -- Expected columns:
    -- entity (text, NO)
    -- view_oid (oid, NO)
    -- table_oid (oid, NO)
    -- definition (text, NO)
    -- dependencies (oid[], NO)
    -- fk_columns (text[], NO)
    -- uuid_fk_columns (text[], NO)
    -- dependency_types (text[], NO)
    -- dependency_paths (text[][], NO)
    -- array_match_keys (text[], NO)
    -- created_at (timestamptz, NO)

ROLLBACK;
```

**Expected Output (failing):**
```
 meta_table_exists
-------------------
 f
```

**GREEN Phase - Implementation:**

```rust
// src/metadata.rs
use pgrx::prelude::*;

pub fn create_metadata_tables() -> Result<(), Box<dyn std::error::Error>> {
    Spi::run(
        r#"
        CREATE TABLE IF NOT EXISTS public.pg_tview_meta (
            entity TEXT NOT NULL PRIMARY KEY,
            view_oid OID NOT NULL,
            table_oid OID NOT NULL,
            definition TEXT NOT NULL,
            dependencies OID[] NOT NULL DEFAULT '{}',
            fk_columns TEXT[] NOT NULL DEFAULT '{}',
            uuid_fk_columns TEXT[] NOT NULL DEFAULT '{}',
            dependency_types TEXT[] NOT NULL DEFAULT '{}',
            dependency_paths TEXT[][] NOT NULL DEFAULT '{}',
            array_match_keys TEXT[] NOT NULL DEFAULT '{}',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        CREATE TABLE IF NOT EXISTS public.pg_tview_helpers (
            helper_name TEXT NOT NULL PRIMARY KEY,
            is_helper BOOLEAN NOT NULL DEFAULT TRUE,
            used_by TEXT[] NOT NULL DEFAULT '{}',
            depends_on TEXT[] NOT NULL DEFAULT '{}',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        COMMENT ON TABLE public.pg_tview_meta IS
            'Metadata for TVIEW materialized tables';
        COMMENT ON TABLE public.pg_tview_helpers IS
            'Tracks helper views used by TVIEWs';
        "#,
    )?;

    Ok(())
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use super::*;

    #[pg_test]
    fn test_metadata_tables_creation() {
        create_metadata_tables().expect("Failed to create metadata tables");

        // Verify pg_tview_meta exists
        let result = Spi::get_one::<bool>(
            "SELECT COUNT(*) = 1 FROM information_schema.tables
             WHERE table_name = 'pg_tview_meta'"
        );
        assert_eq!(result, Ok(Some(true)));

        // Verify pg_tview_helpers exists
        let result = Spi::get_one::<bool>(
            "SELECT COUNT(*) = 1 FROM information_schema.tables
             WHERE table_name = 'pg_tview_helpers'"
        );
        assert_eq!(result, Ok(Some(true)));
    }
}
```

```rust
// src/lib.rs (updated)
use pgrx::prelude::*;

mod metadata;

::pgrx::pg_module_magic!();

#[pg_extern]
fn pg_tviews_version() -> &'static str {
    "0.1.0-alpha"
}

#[pg_guard]
extern "C" fn _PG_init() {
    // Create metadata tables on extension load
    if let Err(e) = metadata::create_metadata_tables() {
        pgrx::error!("Failed to initialize pg_tviews metadata: {}", e);
    }
}
```

**Verify GREEN:**
```bash
cargo pgrx install --release
psql -d test_db -f test/sql/01_metadata_tables.sql
```

**Expected Output:**
```
 meta_table_exists
-------------------
 t

 helpers_table_exists
----------------------
 t

    column_name     |   data_type   | is_nullable
--------------------+---------------+-------------
 entity             | text          | NO
 view_oid           | oid           | NO
 table_oid          | oid           | NO
 ...
```

---

### Test 3: Basic Version Function

**RED Phase - Write Failing Test:**

```rust
// src/lib.rs - Add to tests module
#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_version_function() {
        let version = crate::pg_tviews_version();
        assert!(version.starts_with("0.1.0"));
    }

    #[pg_test]
    fn test_version_callable_from_sql() {
        let result = Spi::get_one::<String>(
            "SELECT pg_tviews_version()"
        );
        assert!(result.is_ok());
        let version = result.unwrap();
        assert!(version.is_some());
        assert!(version.unwrap().starts_with("0.1.0"));
    }
}
```

**Expected Output (may already pass if previous steps complete):**
```
test test_version_function ... ok
test test_version_callable_from_sql ... ok
```

---

## Implementation Steps

### Step 1: Initialize pgrx Project

```bash
# Create new pgrx project
cargo pgrx init
cargo pgrx new pg_tviews

# Navigate to project
cd pg_tviews

# Verify structure
ls -la
# Expected: Cargo.toml, src/, sql/, README.md
```

### Step 2: Configure Dependencies

Edit `Cargo.toml` with configuration shown above (in GREEN phase of Test 1).

### Step 3: Implement Minimal Extension

Create `src/lib.rs` with minimal implementation (Test 1 GREEN phase).

### Step 4: Create SQL Test Infrastructure

```bash
mkdir -p test/sql
mkdir -p test/expected

# Create test files from RED phases above
touch test/sql/00_extension_loading.sql
touch test/sql/01_metadata_tables.sql
```

### Step 5: Build and Test

```bash
# Run Rust unit tests
cargo pgrx test pg17

# Install extension locally
cargo pgrx install --release

# Create test database
createdb pg_tviews_test

# Run SQL tests
psql -d pg_tviews_test -f test/sql/00_extension_loading.sql
psql -d pg_tviews_test -f test/sql/01_metadata_tables.sql
```

### Step 6: Set Up CI/CD

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [ main, develop ]
  pull_request:
    branches: [ main ]

jobs:
  test:
    name: Test on PostgreSQL ${{ matrix.pg_version }}
    runs-on: ubuntu-latest
    strategy:
      matrix:
        pg_version: [15, 16, 17]

    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Install PostgreSQL ${{ matrix.pg_version }}
        run: |
          sudo apt-get update
          sudo apt-get install -y postgresql-${{ matrix.pg_version }} \
                                   postgresql-server-dev-${{ matrix.pg_version }}

      - name: Install pgrx
        run: |
          cargo install --locked cargo-pgrx
          cargo pgrx init --pg${{ matrix.pg_version }} \
            /usr/lib/postgresql/${{ matrix.pg_version }}/bin/pg_config

      - name: Run tests
        run: |
          cargo pgrx test pg${{ matrix.pg_version }}

      - name: Build extension
        run: |
          cargo pgrx install --release
```

---

## Documentation Requirements

### 1. README.md

Create initial README with:
- Project description
- Installation instructions
- Quick start guide
- Development workflow
- Contributing guidelines

### 2. DEVELOPMENT.md

Document:
- Setting up development environment
- Running tests
- Building from source
- Debugging tips
- Code organization

### 3. ARCHITECTURE.md

Initial architecture overview:
- Extension structure
- Metadata table design
- Module organization
- Testing strategy

---

## Acceptance Criteria

### Functional Requirements

- [x] Extension compiles without errors
- [x] Extension installs into PostgreSQL
- [x] Metadata tables created automatically
- [x] Version function callable
- [x] Rust unit tests pass
- [x] SQL integration tests pass

### Quality Requirements

- [x] Code follows Rust style guidelines (rustfmt)
- [x] All tests documented with RED/GREEN phases
- [x] CI pipeline validates all PostgreSQL versions
- [x] Error messages are clear and actionable
- [x] Documentation covers setup and testing

### Performance Requirements

- [x] Extension initialization < 100ms
- [x] Metadata table creation < 50ms
- [x] No memory leaks in initialization code

---

## Rollback Plan

If Phase 0 fails:

1. **Compilation Issues**: Review pgrx version compatibility, check Rust toolchain
2. **PostgreSQL Compatibility**: Test with different PostgreSQL versions, check pg_config
3. **Test Failures**: Review Spi::run error messages, check PostgreSQL logs

No database migrations needed yet - can safely drop extension and retry.

---

## Next Phase

Once Phase 0 is complete and all tests pass:
- **Phase 1**: Schema Inference & Column Detection
- Implement `infer_tview_schema()` function
- Detect `pk_<entity>`, `id`, `fk_*`, `*_id`, `data` columns
- Parse SELECT statement to extract column information

---

## Notes

- Keep this phase minimal - just scaffolding
- Focus on getting TDD infrastructure working
- All subsequent phases build on this foundation
- Do NOT implement TVIEW logic yet - that's Phase 2+

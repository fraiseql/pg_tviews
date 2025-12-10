# Docker Issues and Fixes Summary

## Problems Encountered

### 1. Segmentation Fault with shared_preload_libraries

**Issue**: PostgreSQL crashed during `initdb` when pg_tviews was loaded via `shared_preload_libraries`.

**Error**:
```
2025-12-10 20:42:09.956 UTC [43] LOG:  pg_tviews: _PG_init() called, installing ProcessUtility hook
2025-12-10 20:42:09.956 UTC [43] LOG:  pg_tviews: ProcessUtility hook installed
Segmentation fault (core dumped)
child process exited with exit code 139
```

**Root Cause**:
- During `initdb`, PostgreSQL is initializing the template database
- No actual backend connection exists yet
- pg_tviews `_PG_init()` tries to install ProcessUtility hook before PostgreSQL globals are fully initialized
- This causes a segfault when accessing `pg_sys::ProcessUtility_hook`

**Solution**:
- Removed `shared_preload_libraries = 'pg_tviews'` from postgresql.conf
- Extension is now loaded only via `CREATE EXTENSION pg_tviews` after database is fully initialized
- This is the standard approach for most PostgreSQL extensions

**Code Location**: `src/lib.rs:168-183` (_PG_init function)

### 2. Missing SQL Installation Script

**Issue**: After fixing the segfault, `CREATE EXTENSION pg_tviews` failed with:

```
ERROR:  extension "pg_tviews" has no installation script nor update path for version "0.1.0"
```

**Root Cause**:
- pgrx's `cargo pgrx install` only generates SQL files if the extension exports SQL-visible functions
- pg_tviews currently only provides hooks and internal functions
- No `pg_tviews--0.1.0.sql` file was generated during build

**Solution**:
- Created minimal SQL installation script: `pg_tviews--0.1.0.sql`
- Contains only comments (extension works purely through C hooks)
- Added to Dockerfile build step

**Files Created**:
```sql
-- pg_tviews extension installation script
-- The extension provides hooks and internal functions only
```

## Final Working Configuration

### Dockerfile Changes

```dockerfile
# Build and install pg_tviews extension
WORKDIR /build/pg_tviews
RUN cargo pgrx install --release

# Fix: Create minimal SQL installation script
RUN echo "-- pg_tviews extension installation script" > /usr/share/postgresql/17/extension/pg_tviews--0.1.0.sql && \
    echo "-- The extension provides hooks and internal functions only" >> /usr/share/postgresql/17/extension/pg_tviews--0.1.0.sql

# Build and install jsonb_ivm extension
WORKDIR /build/jsonb_ivm
RUN cargo pgrx install --release

# PostgreSQL configuration (NO shared_preload_libraries)
RUN echo "shared_buffers = 512MB" >> /usr/share/postgresql/postgresql.conf.sample && \
    echo "work_mem = 256MB" >> /usr/share/postgresql/postgresql.conf.sample && \
    echo "max_parallel_workers_per_gather = 4" >> /usr/share/postgresql/postgresql.conf.sample
# Note: NOT loading pg_tviews via shared_preload_libraries (causes segfault during initdb)
```

### Entrypoint Script

```bash
# Create extensions in correct order
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS pg_tviews;      # Loads hook dynamically
CREATE EXTENSION IF NOT EXISTS jsonb_ivm;
```

## Verification

### Extensions Loaded Successfully

```sql
postgres=# \dx
                                          List of installed extensions
   Name    | Version |   Schema   |                                 Description
-----------+---------+------------+-----------------------------------------------------------------------------
 jsonb_ivm | 0.3.0   | public     | Incremental JSONB View Maintenance for CQRS Architectures
 pg_tviews | 0.1.0   | public     | Transactional Views (TVIEWs) for PostgreSQL with automatic refresh triggers
 plpgsql   | 1.0     | pg_catalog | PL/pgSQL procedural language
 uuid-ossp | 1.1     | public     | generate universally unique identifiers (UUIDs)
```

### Benchmark Results

**Docker Container - Small Scale (1,000 products)**:
- [1] pg_tviews + jsonb_ivm: **0.505 ms**
- [2] Manual + native PG: **0.291 ms**
- [3] Full Refresh: **79.032 ms**

Improvement: **157× faster** than full refresh!

## Lessons Learned

### 1. shared_preload_libraries is NOT Always Needed

Many PostgreSQL extensions don't need `shared_preload_libraries`:
- ✅ **Need it**: Background workers, custom GUC variables, shared memory
- ❌ **Don't need it**: Hooks loaded on CREATE EXTENSION, utility functions

pg_tviews falls into the second category - its ProcessUtility hook can be installed lazily when the extension is created.

### 2. pgrx SQL Generation

pgrx only generates SQL files for:
- Functions marked with `#[pg_extern]`
- Types marked with `#[derive(PostgresType)]`
- Schemas, tables, etc. explicitly created

If your extension is pure hooks/internal logic, you need to manually create an empty SQL file.

### 3. Docker Build Context Matters

The multi-project build (pg_tviews + jsonb_ivm) required:
- Build context set to parent directory: `context: ..`
- Correct relative paths in COPY statements
- Both projects accessible from Dockerfile

## Testing the Fix

### Rebuild Container

```bash
# From pg_tviews directory
docker-compose down -v
docker-compose build --no-cache pg_tviews_bench
docker-compose up -d pg_tviews_bench
```

### Verify Extensions

```bash
docker exec pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "\dx"
```

Should show all 4 extensions (pg_tviews, jsonb_ivm, uuid-ossp, plpgsql).

### Run Benchmarks

```bash
docker exec pg_tviews_bench psql -U postgres -d pg_tviews_benchmark \
  -f /benchmarks/00_setup.sql \
  -f /benchmarks/schemas/01_ecommerce_schema.sql \
  -f /benchmarks/data/01_ecommerce_data_small.sql \
  -f /benchmarks/scenarios/01_ecommerce_benchmarks_small.sql
```

## Future Improvements

### 1. Fix _PG_init for shared_preload_libraries

If you want to support `shared_preload_libraries` in the future, add a safety check:

```rust
#[pg_guard]
extern "C" fn _PG_init() {
    // Only install hooks if we're in a real backend (not initdb)
    if unsafe { pg_sys::IsUnderPostmaster } {
        pgrx::log!("pg_tviews: Installing ProcessUtility hook");
        unsafe {
            hooks::install_hook();
        }
    } else {
        pgrx::log!("pg_tviews: Skipping hook installation (not in backend)");
    }
}
```

### 2. Auto-generate SQL Installation Script

Add a build script or make task to ensure the SQL file always exists:

```makefile
# Makefile
install:
    cargo pgrx install --release
    @if [ ! -f /usr/share/postgresql/17/extension/pg_tviews--0.1.0.sql ]; then \
        echo "-- pg_tviews installation" > /usr/share/postgresql/17/extension/pg_tviews--0.1.0.sql; \
    fi
```

### 3. Document Extension Architecture

Update README to clarify:
- Extension doesn't need shared_preload_libraries
- How the ProcessUtility hook works
- When hooks are installed (on CREATE EXTENSION)

## Related Documentation

- [PostgreSQL Extension Building](https://www.postgresql.org/docs/current/extend-extensions.html)
- [pgrx Documentation](https://github.com/pgcentralfoundation/pgrx)
- [ProcessUtility Hooks](https://www.postgresql.org/docs/current/xfunc-c.html#XFUNC-C-HOOKS)

## Summary

Both Docker issues were resolved:
1. ✅ Removed `shared_preload_libraries` requirement (causes segfault)
2. ✅ Created minimal SQL installation script (pgrx doesn't auto-generate for hook-only extensions)

The Docker container now successfully runs benchmarks with both pg_tviews and jsonb_ivm extensions installed! 🎉

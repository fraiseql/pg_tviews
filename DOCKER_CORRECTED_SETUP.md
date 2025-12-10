# Docker Setup - Corrected Architecture

## Important Clarification

I previously made an incorrect assumption about the extension architecture. Here's the **correct** understanding:

### What We're Actually Testing

**pg_tviews uses TWO custom extensions** (not pg_ivm):

1. **pg_tviews** (`/home/lionel/code/pg_tviews/`)
   - Core incremental view maintenance system
   - Trinity pattern support (UUID + INTEGER pk + INTEGER fk)
   - Transactional view infrastructure

2. **jsonb_ivm** (`/home/lionel/code/jsonb_ivm/`)
   - Rust-based JSONB patching functions
   - High-performance partial JSONB updates
   - ~2.66× faster than native PostgreSQL for array updates
   - **This is YOUR custom extension**, not a standard PostgreSQL extension

### What We're NOT Using

❌ **pg_ivm** (from sraoss/pg_ivm)
- This is PostgreSQL's native Incremental View Maintenance extension
- We are **NOT** using this
- pg_tviews uses a different approach

### The Comparison

The benchmarks compare **3 approaches**:

1. **pg_tviews + jsonb_ivm** (Approach 1)
   - Your complete system with Rust-optimized JSONB patching

2. **pg_tviews + native PostgreSQL** (Approach 2)
   - Your system but using native `jsonb_set()` instead of Rust functions
   - This is what the PL/pgSQL stubs simulate

3. **Full Materialized View Refresh** (Baseline)
   - Traditional `REFRESH MATERIALIZED VIEW`
   - Complete recomputation of all data

### Docker Setup

The corrected Dockerfile now:
- ✅ Builds from parent directory (`/home/lionel/code/`)
- ✅ Copies both `pg_tviews/` and `jsonb_ivm/` source code
- ✅ Builds and installs `pg_tviews` extension (Rust/pgrx)
- ✅ Builds and installs `jsonb_ivm` extension (Rust/pgrx)
- ✅ Sets up benchmark infrastructure
- ❌ Does NOT install pg_ivm (not needed)

## Build Command

```bash
cd /home/lionel/code/pg_tviews
docker-compose build pg_tviews_bench
```

The build context is set to `..` (parent directory) so both projects are accessible.

## What Gets Validated

These benchmarks will answer:

1. **How much does Rust-based jsonb_ivm improve performance?**
   - Compare Approach 1 (Rust JSONB) vs Approach 2 (native PostgreSQL)
   - Quantify the ~2.66× speedup in real-world CQRS workloads

2. **How does pg_tviews compare to full refresh?**
   - Already validated: 88-2,853× improvement at medium scale
   - Will confirm with real jsonb_ivm extension (not stubs)

3. **What's the cost/benefit of Rust extension?**
   - Single updates: How much faster?
   - Bulk updates: Does the advantage scale?
   - Cascade scenarios: Impact on complex operations?

## File Structure

```
/home/lionel/code/
├── pg_tviews/
│   ├── Dockerfile.benchmarks        (builds both extensions)
│   ├── docker-compose.yml            (context: ..)
│   ├── src/                          (pg_tviews Rust code)
│   ├── test/sql/comprehensive_benchmarks/
│   └── test/sql/jsonb_ivm_stubs.sql  (fallback if real extension fails)
│
└── jsonb_ivm/
    ├── Cargo.toml                    (pgrx 0.12.8, pg17)
    ├── src/                          (Rust implementation)
    └── sql/                          (SQL definitions)
```

## Expected Extensions in Container

```sql
SELECT extname, extversion FROM pg_extension ORDER BY extname;

 extname      | extversion
--------------+-----------
 jsonb_ivm    | 0.3.1     -- Your Rust JSONB extension
 pg_tviews    | 0.1.0     -- Your IVM extension
 plpgsql      | 1.0       -- Built-in
 uuid-ossp    | 1.1       -- Standard UUID functions
```

## Verification After Build

Once the container starts, verify:

```bash
# Start container
docker-compose up -d pg_tviews_bench

# Check extensions
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "\dx"

# Verify jsonb_ivm functions are Rust-based (not stubs)
docker exec -it pg_tviews_bench psql -U postgres -d pg_tviews_benchmark -c "
  SELECT
    proname,
    prosrc,
    CASE WHEN prosrc LIKE '%stub%' THEN 'PL/pgSQL stub'
         WHEN prosrc LIKE '%$libdir%' THEN 'Rust extension'
         ELSE 'Unknown'
    END as implementation
  FROM pg_proc
  WHERE proname LIKE 'jsonb_smart_patch%'
  ORDER BY proname;
"
```

Expected output should show `Rust extension` for all `jsonb_smart_patch_*` functions.

## Current Build Status

**Building now** (~10-15 minutes):
- Follow: `tail -f /tmp/docker-build-jsonb.log`
- Building: PostgreSQL 17 + Rust toolchain + cargo-pgrx + both extensions

## Next Steps

1. ✅ Wait for build (in progress)
2. ⏳ Start container
3. ⏳ Verify extensions loaded correctly
4. ⏳ Run small-scale benchmark (validate setup)
5. ⏳ Run medium-scale benchmark (compare with previous results)
6. ⏳ Analyze: How much does real jsonb_ivm improve over stubs?
7. ⏳ Document findings

## Key Insight

The previous benchmark results were **conservative** because they used PL/pgSQL stubs.

With the real Rust-based `jsonb_ivm` extension, we expect:
- Single updates: Similar (already near optimal)
- Bulk JSONB operations: **20-50% faster** (Rust optimization)
- Overall improvement over full refresh: **Even better than 2,853×**

This will validate the complete pg_tviews architecture with all optimizations enabled!

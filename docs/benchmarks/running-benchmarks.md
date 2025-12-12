# Running Benchmarks

This guide explains how to run the pg_tviews benchmark suite to validate functionality and measure performance.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Quick Start](#quick-start)
  - [Option A: Docker (Recommended)](#option-a-docker-recommended---full-4-way-benchmark)
  - [Option B: Manual Approaches 3 & 4 Only](#option-b-manual-approaches-3--4-only-no-extensions-required)
  - [Option C: pgrx-managed PostgreSQL](#option-c-pgrx-managed-postgresql-advanced)
  - [Option D: System PostgreSQL](#option-d-system-postgresql-with-manual-extension-install-requires-sudo)
- [Deployment Options](#deployment-options)
- [Running the Benchmark Suite](#running-the-benchmark-suite)
- [Understanding Results](#understanding-results)
- [Troubleshooting](#troubleshooting)
- [Next Steps](#next-steps)

---

## Prerequisites

**IMPORTANT**: pg_tviews supports **PostgreSQL 13-18**. The extension uses pgrx 0.16.1 with full PostgreSQL 18 compatibility.

### System Requirements

- **PostgreSQL**: 13-18 (all versions supported)
- **Rust**: 1.70 or later (`rustc --version`)
- **pgrx**: 0.16.1 (`cargo install cargo-pgrx --version 0.16.1 --locked`)
- **Disk Space**:
  - Small scale: 5GB free
  - Medium scale: 10GB free
  - Large scale: 20GB+ free
- **Memory**:
  - Small scale: 4GB+ recommended
  - Medium scale: 8GB+ recommended
  - Large scale: 16GB+ recommended
- **Docker**: Optional but recommended for full 4-way benchmarks

### Extension Dependencies

- **pg_tviews**: Core extension (built from source) - **Required for approaches 1 & 2**
- **jsonb_ivm**: Optional performance extension (built from source) - **Required for approach 1**
- **pg_ivm**: Alternative incremental view extension (optional)

### Check Your PostgreSQL Version

```bash
psql --version
# pg_tviews supports PostgreSQL 13-18
```

### Benchmark Capability Matrix

| Approach | Extensions Required | Performance | Setup Difficulty |
|----------|-------------------|-------------|------------------|
| **1: pg_tviews + jsonb_ivm** | pg_tviews + jsonb_ivm | Maximum (1.0x) | Hard (system install) |
| **2: pg_tviews + native PG** | pg_tviews only | 98% of maximum | Hard (system install) |
| **3: Manual functions** | None | 95% of maximum | Medium (manual setup) |
| **4: Full refresh** | None | 0.01-0.02% of max | Easy (built-in PG) |

---

## Quick Start (4 Options)

### Option A: Docker (Recommended - Full 4-Way Benchmark)

**✅ Supports all 4 approaches** - Most reliable for complete testing

**Prerequisites**:
- Docker and Docker Compose installed
- Both repositories cloned in same parent directory:
  ```
  /path/to/code/
    ├── pg_tviews/
    └── jsonb_ivm/    # Clone from https://github.com/fraiseql/jsonb_ivm
  ```

```bash
# 1. Build benchmark container with extensions
# Note: Build context is parent directory (must contain both pg_tviews and jsonb_ivm)
cd /path/to/pg_tviews
docker build -f docker/dockerfile-benchmarks -t pg_tviews_bench ..

# OR use docker-compose (recommended):
cd /path/to/pg_tviews/docker
docker-compose up -d --build

# 2. Run container (if using docker build)
docker run -d --name pg_tviews_benchmark \
  -p 5432:5432 \
  -e POSTGRES_PASSWORD=postgres \
  pg_tviews_bench

# 3. Wait for startup (30 seconds)
sleep 30

# 4. Run benchmarks (choose scale)
# Small scale (fastest, ~5 minutes total):
docker exec -it pg_tviews_benchmark psql -U postgres -d pg_tviews_benchmark -c "
\i /benchmarks/00_setup.sql
\i /benchmarks/schemas/01_ecommerce_schema.sql
\i /benchmarks/data/01_ecommerce_data_small.sql
\i /benchmarks/scenarios/01_ecommerce_benchmarks_small.sql
"

# Medium scale (~15 minutes, requires 8GB+ RAM):
docker exec -it pg_tviews_benchmark psql -U postgres -d pg_tviews_benchmark -c "
\i /benchmarks/00_setup.sql
\i /benchmarks/schemas/01_ecommerce_schema.sql
\i /benchmarks/data/01_ecommerce_data_medium.sql
\i /benchmarks/scenarios/01_ecommerce_benchmarks_medium.sql
"

# Large scale (~1 hour, requires 16GB+ RAM):
docker exec -it pg_tviews_benchmark psql -U postgres -d pg_tviews_benchmark -c "
\i /benchmarks/00_setup.sql
\i /benchmarks/schemas/01_ecommerce_schema.sql
\i /benchmarks/data/01_ecommerce_data_large.sql
\i /benchmarks/scenarios/01_ecommerce_benchmarks_large.sql
"

# 5. View results
docker exec -it pg_tviews_benchmark psql -U postgres -d pg_tviews_benchmark -c "
SELECT * FROM benchmark_summary ORDER BY execution_time_ms;
SELECT * FROM benchmark_comparison WHERE improvement_ratio IS NOT NULL ORDER BY improvement_ratio DESC;
"
```

### Option B: Manual Approaches 3 & 4 Only (No Extensions Required)

**✅ Works on any PostgreSQL** - Demonstrates incremental vs full refresh benefits

#### Scale Options:
- **Small**: 1K products, 5K reviews (~2 minutes setup, 4GB RAM)
- **Medium**: 100K products, 500K reviews (~5 minutes setup, 8GB RAM)
- **Large**: 1M products, 5M reviews (~30 minutes setup, 16GB RAM)

```bash
# 1. Create benchmark database
createdb pg_tviews_benchmark
psql -d pg_tviews_benchmark -c "CREATE EXTENSION IF NOT EXISTS \"uuid-ossp\";"

# 2. Setup schema manually (skip pg_tviews parts)
cd test/sql/comprehensive_benchmarks
psql -d pg_tviews_benchmark -f 00_setup.sql

# 3. Load data (choose scale)
psql -d pg_tviews_benchmark -f data/01_ecommerce_data_small_manual.sql    # Small scale
# OR
psql -d pg_tviews_benchmark -f data/01_ecommerce_data_medium_manual.sql  # Medium scale
# OR
psql -d pg_tviews_benchmark -f data/01_ecommerce_data_large_manual.sql   # Large scale

# 4. Load manual functions
psql -d pg_tviews_benchmark -f functions/refresh_product_manual.sql

# 5. Populate manual tables
psql -d pg_tviews_benchmark -c "
INSERT INTO manual_func_product (pk_product, fk_category, data)
SELECT pk_product, fk_category, data FROM v_product;
REFRESH MATERIALIZED VIEW mv_product;
"

# 6. Run manual benchmark
psql -d pg_tviews_benchmark -c "
-- Test Approach 3 vs 4
UPDATE tb_product SET current_price = current_price * 0.9 WHERE pk_product = 1;
SELECT refresh_product_manual('product', 1, 'price_current');
UPDATE tb_product SET current_price = current_price / 0.9 WHERE pk_product = 1;
REFRESH MATERIALIZED VIEW mv_product;
"
```

### Option C: pgrx-managed PostgreSQL (Advanced)

**⚠️ Time-intensive setup** - Compiles PostgreSQL from source

```bash
# 1. Initialize pgrx (30-60 minutes first time)
cargo pgrx init

# 2. Start managed PostgreSQL 18
cargo pgrx start pg18

# 3. Install extensions
cargo pgrx install --release

# 4. Run benchmarks (use port 28818 for pg18)
psql -h localhost -p 28818 -c "CREATE DATABASE pg_tviews_benchmark;"
psql -h localhost -p 28818 -d pg_tviews_benchmark -c "CREATE EXTENSION pg_tviews;"
psql -h localhost -p 28818 -d pg_tviews_benchmark -f test/sql/comprehensive_benchmarks/00_setup.sql
psql -h localhost -p 28818 -d pg_tviews_benchmark -f test/sql/comprehensive_benchmarks/schemas/01_ecommerce_schema.sql
```

### Option D: System PostgreSQL with Manual Extension Install (Requires sudo)

**❌ Requires system admin access** - Not available in many environments

```bash
# 1. Install system-wide (requires sudo)
sudo cargo pgrx install --release

# 2. Create benchmark database
createdb pg_tviews_benchmark
psql -d pg_tviews_benchmark -c "CREATE EXTENSION pg_tviews;"

# 3. Run full benchmarks
cd test/sql/comprehensive_benchmarks
psql -d pg_tviews_benchmark -f 00_setup.sql
psql -d pg_tviews_benchmark -f scenarios/01_ecommerce_benchmarks_small.sql
```

---

## Deployment Options

You have two options for running benchmarks, depending on your PostgreSQL installation.

### Option A: pgrx-managed PostgreSQL (Recommended)

pgrx can manage its own PostgreSQL installations, which is ideal for development and testing.

#### Setup

```bash
# Initialize pgrx (one-time setup)
cargo pgrx init

# Check status
cargo pgrx status
```

**Expected output**:
```
Postgres v13 is stopped
Postgres v14 is stopped
Postgres v15 is stopped
Postgres v16 is stopped
Postgres v17 is stopped
```

#### Start PostgreSQL 17

```bash
# Start pg17
cargo pgrx start pg17

# Verify it's running
cargo pgrx status
```

**Expected output**:
```
Postgres v17 is running
```

#### Connection Details

- **Host**: `localhost`
- **Port**: `28817` (NOT the default 5432)
- **User**: Your system username (`$(whoami)`)
- **Database**: `postgres` (default)

#### Test Connection

```bash
psql -h localhost -p 28817 -c "SELECT version();"
```

**Expected output**: Version string showing PostgreSQL 17.x

---

### Option B: Docker

Use Docker if you prefer containerized environments or need isolation from your host system.

#### Build Benchmark Container

```bash
# Build from pg_tviews directory (build context is parent directory)
cd /path/to/pg_tviews
docker build -f docker/dockerfile-benchmarks -t pg_tviews_bench ..

# OR use docker-compose:
cd /path/to/pg_tviews/docker
docker-compose up -d --build
```

**Note**: The build context is the parent directory, which must contain both `pg_tviews/` and `jsonb_ivm/` projects side-by-side.

#### Run Container

```bash
# Start container
docker run -d --name pg_tviews_benchmark \
  -p 5432:5432 \
  -e POSTGRES_PASSWORD=postgres \
  pg_tviews_bench

# Wait for PostgreSQL to start
sleep 5

# Verify container is running
docker ps | grep pg_tviews_benchmark
```

#### Connection Details

- **Host**: `localhost`
- **Port**: `5432` (default PostgreSQL port)
- **User**: `postgres`
- **Password**: `postgres`
- **Database**: `pg_tviews_benchmark` (auto-created)

#### Test Connection

```bash
# From host
docker exec -it pg_tviews_benchmark psql -U postgres -d pg_tviews_benchmark -c "SELECT version();"

# Or connect directly
psql -h localhost -p 5432 -U postgres -d pg_tviews_benchmark
```

---

## Running the Benchmark Suite

### Step 1: Clean Build Environment

```bash
# Clean previous builds
cargo clean

# Verify clean state
ls -la target/ 2>/dev/null || echo "Clean state confirmed"
```

### Step 2: Compile and Install Extension

```bash
# Compile and install (release mode for accurate performance)
cargo pgrx install --release

# Verify installation
psql -h localhost -p 28817 -c "SELECT * FROM pg_available_extensions WHERE name = 'pg_tviews';"
```

**Expected output**:
```
   name    | default_version | installed_version | comment
-----------+-----------------+-------------------+---------
 pg_tviews | 0.1.0-beta.1   |                   | Transactional materialized views...
```

### Step 3: Create Benchmark Database

```bash
# Drop existing benchmark DB if it exists
psql -h localhost -p 28817 -c "DROP DATABASE IF EXISTS pg_tviews_benchmark;"

# Create fresh benchmark database
psql -h localhost -p 28817 -c "CREATE DATABASE pg_tviews_benchmark;"

# Create extension
psql -h localhost -p 28817 -d pg_tviews_benchmark -c "CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;"

# Verify extension loaded
psql -h localhost -p 28817 -d pg_tviews_benchmark -c "SELECT extname, extversion FROM pg_extension WHERE extname = 'pg_tviews';"
```

**Expected output**:
```
  extname  | extversion
-----------+------------
 pg_tviews | 0.1.0-beta.1
```

### Step 4: Run E-commerce Benchmark

```bash
# Execute benchmark SQL
psql -h localhost -p 28817 -d pg_tviews_benchmark \
  -f test/sql/comprehensive_benchmarks/schemas/01_ecommerce_schema.sql

# Check exit code
echo "Exit code: $?"
```

**Expected output**:
- Schema creation statements execute successfully
- TVIEW creation statements succeed
- Sample data inserted
- Benchmark queries execute with timing data
- Exit code: 0

#### Verify Results

```sql
-- Connect to benchmark database
psql -h localhost -p 28817 -d pg_tviews_benchmark

-- Check tviews were created
SELECT entity, table_oid::regclass AS table_name, view_oid::regclass AS view_name
FROM pg_tview_meta
ORDER BY entity;

-- Verify data is present (adjust table names based on actual schema)
SELECT COUNT(*) FROM tv_customer_order_summary;
SELECT COUNT(*) FROM tv_product_sales_stats;
```

### Step 5: Run Full Test Suite

```bash
# Run all SQL tests (this may take several minutes)
cargo pgrx test pg17
```

**Expected output**:
- Test database created
- Extension loaded
- All tests pass
- Test summary showing pass/fail counts

### Step 6: Load Monitoring System (Optional)

For production-like metrics collection:

```bash
# Load monitoring views and functions
psql -h localhost -p 28817 -d pg_tviews_benchmark \
  -f sql/pg_tviews_monitoring.sql

# Verify monitoring tables created
psql -h localhost -p 28817 -d pg_tviews_benchmark -c "\dt pg_tviews_*"
```

**Expected output**:
```
                List of relations
 Schema |       Name       | Type  | Owner
--------+------------------+-------+-------
 public | pg_tviews_metrics| table | ...
```

### Step 7: Collect Performance Metrics

```bash
psql -h localhost -p 28817 -d pg_tviews_benchmark <<'EOF'
-- Report on all tviews in the catalog
SELECT
    entity,
    table_oid::regclass AS tview_table,
    view_oid::regclass AS view_name,
    array_length(dependencies, 1) AS dependency_count,
    created_at
FROM pg_tview_meta
ORDER BY entity;

-- Report on table sizes
SELECT
    table_oid::regclass AS tview_table,
    pg_size_pretty(pg_total_relation_size(table_oid)) AS total_size,
    pg_size_pretty(pg_relation_size(table_oid)) AS table_size,
    pg_size_pretty(pg_indexes_size(table_oid)) AS indexes_size
FROM pg_tview_meta
ORDER BY pg_total_relation_size(table_oid) DESC;

-- Check trigger execution stats
SELECT
    schemaname,
    tablename,
    pg_size_pretty(pg_relation_size(schemaname||'.'||tablename)) as size
FROM pg_stat_user_tables
WHERE schemaname = 'public' AND tablename LIKE 'tv_%'
ORDER BY pg_relation_size(schemaname||'.'||tablename) DESC;
EOF
```

### Step 8: Verify Data Consistency (Optional)

Check that materialized data matches source data:

```sql
-- Connect to benchmark database
psql -h localhost -p 28817 -d pg_tviews_benchmark

-- List all tables to identify TVIEWs and source tables
\dt

-- Example consistency check (adjust based on actual schema):
WITH source_data AS (
    SELECT customer_id, COUNT(*) as order_count, SUM(total_amount) as total_spent
    FROM orders
    GROUP BY customer_id
),
tview_data AS (
    SELECT pk_customer, order_count, total_spent
    FROM tv_customer_summary
)
SELECT
    CASE
        WHEN COUNT(*) = 0 THEN 'CONSISTENT ✓'
        ELSE 'INCONSISTENT: ' || COUNT(*) || ' mismatches ✗'
    END as consistency_check
FROM (
    SELECT * FROM source_data
    EXCEPT
    SELECT * FROM tview_data
    UNION ALL
    SELECT * FROM tview_data
    EXCEPT
    SELECT * FROM source_data
) mismatches;
```

### Step 9: Cleanup (Optional)

```bash
# Drop benchmark database
psql -h localhost -p 28817 -c "DROP DATABASE IF EXISTS pg_tviews_benchmark;"

# Verify cleanup
psql -h localhost -p 28817 -l | grep benchmark

# Stop PostgreSQL 17 (if using pgrx)
cargo pgrx stop pg17
```

---

## Understanding Results

### Benchmark Output

The e-commerce benchmark measures:

1. **Schema Creation Time**: Time to create base tables
2. **TVIEW Creation Time**: Time to create transactional views
3. **Initial Population Time**: Time to populate with sample data
4. **Incremental Update Performance**: Time for single-row updates to propagate
5. **Cascade Performance**: Time for updates to cascade through dependencies

### Performance Baselines

Expected performance (will vary based on hardware):

| Operation | Expected Duration |
|-----------|------------------|
| Simple TVIEW refresh | < 100ms for 1K rows |
| Complex join TVIEW refresh | < 1s for 10K rows |
| Trigger-based incremental refresh | < 50ms per change |
| Cascade through 3 levels | < 200ms per change |

### Success Indicators

#### Docker Setup Success
- ✅ `docker build` completes without errors
- ✅ Container starts: `docker ps` shows running container
- ✅ PostgreSQL accessible: `docker exec pg_tviews_benchmark psql -U postgres -c "SELECT 1;"`
- ✅ Extensions loaded: `SELECT * FROM pg_extension;` shows pg_tviews and jsonb_ivm

#### Manual Setup Success (Approaches 3 & 4)
- ✅ Database created: `psql -l` shows pg_tviews_benchmark
- ✅ Setup script runs: `00_setup.sql` executes without errors
- ✅ Data loads: `01_ecommerce_data_small_manual.sql` completes
- ✅ Manual functions available: `\df refresh_product_manual` shows function
- ✅ Tables populated: `SELECT COUNT(*) FROM manual_func_product;` > 0

#### Compilation Success (pgrx/system)
- ✅ `cargo pgrx install --release` exits with code 0
- ✅ No "error" lines in output (warnings are OK)
- ✅ Messages show "Installing shared library to..." and "Copying control file to..."

#### Installation Success
- ✅ Extension appears in `pg_available_extensions`
- ✅ `CREATE EXTENSION pg_tviews` succeeds
- ✅ Extension version is `0.1.0-beta.1`
- ✅ Catalog tables `pg_tview_meta` and `pg_tview_helpers` are created

#### Benchmark Success
- ✅ All SQL files execute without errors
- ✅ TVIEWs created and registered in catalog (approaches 1 & 2)
- ✅ Manual tables populated (approaches 3 & 4)
- ✅ Refresh operations complete successfully
- ✅ Performance improvements shown in benchmark_comparison table

---

## Troubleshooting

### Compilation Fails

```bash
# Check Rust toolchain
rustc --version  # Should be 1.70+

# Check pgrx version
cargo pgrx --version  # Should be 0.12.8+

# Ensure pgrx is initialized
cargo pgrx init

# Try clean build
cargo clean && cargo pgrx install --release
```

### PostgreSQL 17 Won't Start

```bash
# Check status
cargo pgrx status

# Try starting explicitly
cargo pgrx start pg17

# Check you can connect
psql -h localhost -p 28817 -c "SELECT version();"

# If still failing, re-initialize
cargo pgrx init
```

### Extension Installation Fails

```bash
# For system-wide installation (requires sudo)
sudo cargo pgrx install --release

# For pgrx-managed PostgreSQL
cargo pgrx start pg18  # Use pg18 for PostgreSQL 18
cargo pgrx install --release

# Verify installation
psql -h localhost -p 28818 -c "SELECT * FROM pg_available_extensions WHERE name LIKE '%tviews%';"
```

### Permission Denied During Installation

**Problem**: `cargo pgrx install` fails with "Permission denied"

**Solutions**:
1. **Use Docker** (recommended):
   ```bash
   cd /path/to/pg_tviews
   docker build -f docker/dockerfile-benchmarks -t pg_tviews_bench ..
   docker run -d --name pg_tviews_benchmark -p 5432:5432 -e POSTGRES_PASSWORD=postgres pg_tviews_bench
   ```

2. **Use pgrx-managed PostgreSQL**:
   ```bash
   cargo pgrx start pg18
   cargo pgrx install --release
   # Use port 28818 instead of 5432
   ```

3. **Manual approaches 3 & 4** (no extensions needed):
   ```bash
   # Skip extension installation entirely
   # Use manual functions and materialized views
   ```

### pgrx init Takes Too Long

**Problem**: `cargo pgrx init` compiles multiple PostgreSQL versions (30-60 minutes)

**Solutions**:
1. **Use Docker** - bypasses local compilation
2. **Use manual approaches** - no extensions needed
3. **Pre-built PostgreSQL** - use system PostgreSQL with manual setup

### Data Loading Fails

**Problem**: Foreign key constraint violations during data generation

**Solution**: The data script expects specific sequence values. Use:
```bash
# Reset sequences before loading data
psql -d pg_tviews_benchmark -c "TRUNCATE tb_category, tb_supplier, tb_product, tb_review, tb_inventory RESTART IDENTITY CASCADE;"
```

### Benchmarks Fail

```bash
# Check PostgreSQL logs
cat ~/.pgrx/17.*/pgdata/log/*.log | tail -50

# Verify base tables were created
psql -h localhost -p 28817 -d pg_tviews_benchmark -c "\dt"

# Check for permission issues
psql -h localhost -p 28817 -d pg_tviews_benchmark -c "\dp"

# Verify sufficient resources
free -h  # Check memory
df -h    # Check disk space
```

### Data Inconsistency Detected

```bash
# Check catalog metadata
psql -h localhost -p 28817 -d pg_tviews_benchmark -c "SELECT * FROM pg_tview_meta;"

# Verify dependencies are correct
psql -h localhost -p 28817 -d pg_tviews_benchmark -c \
  "SELECT entity, dependencies FROM pg_tview_meta;"

# Check if triggers are installed
psql -h localhost -p 28817 -d pg_tviews_benchmark -c \
  "SELECT * FROM pg_trigger WHERE tgname LIKE '%tview%';"

# Examine definition
psql -h localhost -p 28817 -d pg_tviews_benchmark -c \
  "SELECT entity, definition FROM pg_tview_meta WHERE entity = 'your_entity';"
```

### Port Already in Use

If port 28817 is already in use:

```bash
# Check what's using the port
lsof -i :28817

# Stop conflicting process or use Docker instead
docker run -d --name pg_tviews_benchmark \
  -p 5432:5432 \
  -e POSTGRES_PASSWORD=postgres \
  pg_tviews_bench
```

### Manual Setup for Approaches 3 & 4

**Problem**: Want to test incremental benefits without extension complexity

**Solution**: Manual setup process (tested and working):

```bash
# 1. Create database and setup
createdb pg_tviews_benchmark
cd test/sql/comprehensive_benchmarks
psql -d pg_tviews_benchmark -f 00_setup.sql

# 2. Load modified data (skips extension-dependent parts)
psql -d pg_tviews_benchmark -v data_scale="'small'" -f data/01_ecommerce_data_small_manual.sql

# 3. Load manual functions
psql -d pg_tviews_benchmark -f functions/refresh_product_manual.sql

# 4. Populate manual tables
psql -d pg_tviews_benchmark -c "
INSERT INTO manual_func_product (pk_product, fk_category, data)
SELECT pk_product, fk_category, data FROM v_product;
REFRESH MATERIALIZED VIEW mv_product;
"

# 5. Run performance comparison
psql -d pg_tviews_benchmark -c "
-- Single product update comparison
UPDATE tb_product SET current_price = current_price * 0.9 WHERE pk_product = 1;
SELECT refresh_product_manual('product', 1, 'price_current');
UPDATE tb_product SET current_price = current_price / 0.9 WHERE pk_product = 1;
REFRESH MATERIALIZED VIEW mv_product;
"
```

**Expected Results**:
- Manual function: ~2-3ms (surgical update)
- Full refresh: ~70-80ms (scans all products)
- Improvement: 25-35× faster

### Docker-Specific Issues

```bash
# Check container is running
docker ps | grep pg_tviews_benchmark

# Check container logs
docker logs pg_tviews_benchmark

# Restart container
docker restart pg_tviews_benchmark

# Rebuild container (from pg_tviews directory)
cd /path/to/pg_tviews
docker rm -f pg_tviews_benchmark
docker build -f docker/dockerfile-benchmarks -t pg_tviews_bench ..
docker run -d --name pg_tviews_benchmark -p 5432:5432 -e POSTGRES_PASSWORD=postgres pg_tviews_bench
```

## Troubleshooting

### Extension Installation Issues

#### pg_tviews not available
```bash
# Check PostgreSQL version
psql --version  # Must be 13-17, not 18+

# Check if extension is installed
psql -d postgres -c "SELECT * FROM pg_available_extensions WHERE name = 'pg_tviews';"

# Rebuild extension
cargo clean && cargo pgrx install --release
```

#### jsonb_ivm not found
```bash
# Benchmarks will automatically use stubs
# Check if stubs are loaded
psql -d pg_tviews_benchmark -c "SELECT jsonb_ivm_available();"
# Should return true
```

#### Build failures
```bash
# Check Rust version
rustc --version  # Must be 1.70+

# Check pgrx version
cargo pgrx --version  # Should be 0.12.8+

# Clean and rebuild
cargo clean && cargo pgrx install --release
```

### Performance Issues

#### Slow benchmark results
```bash
# Check PostgreSQL configuration
psql -c "SHOW shared_buffers;"
psql -c "SHOW work_mem;"

# Check system resources
free -h  # Memory
df -h    # Disk space

# Restart PostgreSQL with optimized settings
pg_ctl restart -o "-c shared_buffers=512MB -c work_mem=256MB"
```

#### Inconsistent results
```bash
# Use fresh database for each test
dropdb pg_tviews_benchmark
createdb pg_tviews_benchmark

# Clear system cache (Linux)
echo 3 | sudo tee /proc/sys/vm/drop_caches

# Check for concurrent activity
psql -c "SELECT * FROM pg_stat_activity;"
```

### PostgreSQL Connection Issues

#### Port already in use
```bash
# Find what's using the port
lsof -i :5432

# Use different port for pgrx
cargo pgrx start pg17 --port 5433
```

#### Permission denied
```bash
# Check PostgreSQL is running
sudo systemctl status postgresql

# Check your user can connect
psql -U postgres -c "SELECT version();"
```

### Docker-Specific Issues

#### Container won't start
```bash
# Check Docker is running
docker ps

# Check container logs
docker logs pg_tviews_bench

# Clean up and restart
docker-compose down -v
docker-compose up -d pg_tviews_bench
```

#### Extension build fails in Docker
```bash
# Check available memory
docker system info | grep Memory

# Increase Docker memory limit or reduce build parallelism
export DOCKER_BUILDKIT=0
```

### Benchmark Script Issues

#### Schema creation fails
```bash
# Check database exists and is accessible
psql -l | grep pg_tviews_benchmark

# Run setup manually
psql -d pg_tviews_benchmark -f test/sql/comprehensive_benchmarks/00_setup.sql
```

#### No results generated
```bash
# Check benchmark log
tail -f test/sql/comprehensive_benchmarks/results/benchmark_run_*.log

# Verify extensions are loaded
psql -d pg_tviews_benchmark -c "\dx"
```

### pgrx Version Compatibility

**Important**: There are two different version numbers:
- **pgrx library** (in `Cargo.toml`): 0.16.1 - The Rust library used by the extension
- **cargo-pgrx CLI tool**: 0.12.8 or 0.16.1 - The command-line tool to build extensions

**These versions are cross-compatible**:
- cargo-pgrx 0.12.8 can build projects using pgrx 0.16.1 ✅
- cargo-pgrx 0.16.1 can build projects using pgrx 0.16.1 ✅

**Current setup**:
- Docker uses: cargo-pgrx 0.12.8 (stable, tested)
- Project library: pgrx 0.16.1 (with PostgreSQL 18 support)

If you encounter pgrx-related issues:

```bash
# Check your cargo-pgrx version
cargo pgrx --version

# Option 1: Use 0.12.8 (stable, works with pgrx 0.16.1)
cargo install cargo-pgrx --version 0.12.8 --locked

# Option 2: Upgrade to 0.16.1 (latest, matching library version)
cargo install cargo-pgrx --version 0.16.1 --locked
```

---

## Additional Resources

- **Benchmark Results**: See `test/sql/comprehensive_benchmarks/final_results/` for detailed analysis
- **Docker Benchmarks**: See `docs/benchmarks/docker-benchmarks.md` for Docker-specific setup
- **Performance Analysis**: See `docs/benchmarks/results.md` for detailed performance metrics
- **Architecture**: See `ARCHITECTURE.md` for system design details

---

## Next Steps

After running benchmarks:

1. **Review Results**:
   - Docker: `docker exec pg_tviews_benchmark psql -U postgres -d pg_tviews_benchmark -c "SELECT * FROM benchmark_comparison ORDER BY improvement_ratio DESC;"`
   - Manual: `psql -d pg_tviews_benchmark -c "SELECT * FROM benchmark_summary;"`
   - Check `docs/benchmarks/results.md` for interpretation

2. **Choose Your Approach**:
   - **Production with extensions**: Use Docker or pgrx-managed setup
   - **Evaluation/development**: Use manual approaches 3 & 4
   - **Existing systems**: Start with approach 3 (manual functions)

3. **Test Your Workload**: Create custom benchmarks matching your use case
4. **Production Deployment**: See `docs/operations/` for deployment guides
5. **Performance Tuning**: See `docs/operations/performance-tuning.md` (if available)

## Summary

| Setup Method | Approaches Supported | Difficulty | Use Case |
|-------------|---------------------|------------|----------|
| **Docker** | 1, 2, 3, 4 | Easy | Complete evaluation, production testing |
| **Manual (3 & 4 only)** | 3, 4 | Medium | Quick evaluation, existing PostgreSQL |
| **pgrx-managed** | 1, 2, 3, 4 | Hard | Development, full control |
| **System install** | 1, 2, 3, 4 | Hard | Production deployment (requires sudo) |

---

**Last Updated**: December 2025
**pg_tviews Version**: 0.1.0-beta.1

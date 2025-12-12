# Running Benchmarks

This guide explains how to run the pg_tviews benchmark suite to validate functionality and measure performance.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Quick Start](#quick-start)
- [Deployment Options](#deployment-options)
  - [Option A: pgrx-managed PostgreSQL (Recommended)](#option-a-pgrx-managed-postgresql-recommended)
  - [Option B: Docker](#option-b-docker)
- [Running the Benchmark Suite](#running-the-benchmark-suite)
- [Understanding Results](#understanding-results)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

**IMPORTANT**: pg_tviews requires **PostgreSQL 13-17**. The extension uses pgrx 0.16.1, which does **NOT** support PostgreSQL 18+.

### System Requirements

- **Rust**: 1.70 or later (`rustc --version`)
- **pgrx**: 0.12.8+ (`cargo install cargo-pgrx`)
- **Disk Space**: At least 5GB free
- **Memory**: 4GB+ recommended for benchmarks

### Check Your PostgreSQL Version

```bash
psql --version
# If you have pg18+, you'll need to use pgrx's managed pg17 or Docker
```

---

## Quick Start

For the impatient:

```bash
# 1. Start PostgreSQL 17 (pgrx-managed)
cargo pgrx start pg17

# 2. Install extension
cargo pgrx install --release

# 3. Run test suite
cargo pgrx test pg17

# 4. Run e-commerce benchmark
psql -h localhost -p 28817 -c "CREATE DATABASE pg_tviews_benchmark;"
psql -h localhost -p 28817 -d pg_tviews_benchmark -c "CREATE EXTENSION pg_tviews;"
psql -h localhost -p 28817 -d pg_tviews_benchmark -f test/sql/comprehensive_benchmarks/schemas/01_ecommerce_schema.sql
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
# Build from parent directory (requires both pg_tviews and jsonb_ivm)
docker build -f Dockerfile.benchmarks -t pg_tviews_bench ..
```

**Note**: The Dockerfile expects the parent directory to contain both `pg_tviews/` and `jsonb_ivm/` projects.

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

#### Compilation Success
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
- ✅ TVIEWs created and registered in catalog
- ✅ Refresh operations complete successfully
- ✅ Data consistency checks pass

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
# Verify PostgreSQL is running
cargo pgrx status  # Should show "Postgres v17 is running"

# Verify you can connect
psql -h localhost -p 28817 -c "SELECT 1;"

# Check pgrx knows about pg17
cargo pgrx status

# Re-install
cargo pgrx install --release --pg-config $(which pg_config)
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

### Docker-Specific Issues

```bash
# Check container is running
docker ps | grep pg_tviews_benchmark

# Check container logs
docker logs pg_tviews_benchmark

# Restart container
docker restart pg_tviews_benchmark

# Rebuild container
docker rm -f pg_tviews_benchmark
docker build -f Dockerfile.benchmarks -t pg_tviews_bench ..
docker run -d --name pg_tviews_benchmark -p 5432:5432 -e POSTGRES_PASSWORD=postgres pg_tviews_bench
```

### pgrx Version Mismatch

The project uses pgrx 0.16.1 in `Cargo.toml`, but you may have cargo-pgrx 0.12.8 installed. This is **compatible** - cargo-pgrx 0.12.x can build pgrx 0.16.x projects.

If you encounter issues:

```bash
# Upgrade cargo-pgrx
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

1. **Review Results**: Check `docs/benchmarks/results.md` for interpretation
2. **Test Your Workload**: Create custom benchmarks matching your use case
3. **Production Deployment**: See `docs/operations/` for deployment guides
4. **Performance Tuning**: See `docs/operations/performance-tuning.md` (if available)

---

**Last Updated**: December 2025
**pg_tviews Version**: 0.1.0-beta.1

# Installation Guide

Complete installation instructions for pg_tviews in different environments.

## System Requirements

- **PostgreSQL**: 13, 14, 15, 16, 17, or 18
- **Rust**: 1.70+ (for building from source)
- **Memory**: 2GB RAM minimum, 4GB recommended
- **Disk**: 500MB free space for build artifacts

## Quick Install (Recommended)

### 1. Install Rust

```bash
# Install Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Verify installation
rustc --version  # Should show 1.70+
cargo --version  # Should show 1.70+
```

### 2. Install pgrx

```bash
# Install pgrx PostgreSQL extension framework
cargo install --locked cargo-pgrx

# Initialize pgrx with your PostgreSQL version
cargo pgrx init
```

### 3. Build and Install pg_tviews

```bash
# Clone the repository
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews

# Build and install (release mode for production)
cargo pgrx install --release
```

### 4. Enable in Database

```sql
-- Connect to your database
psql -d your_database

-- Enable the extension
CREATE EXTENSION pg_tviews;

-- Verify installation
SELECT pg_tviews_version();

-- Check jsonb_delta status (optional performance enhancement)
SELECT pg_tviews_check_jsonb_delta();
```

## Optional: Install jsonb_delta for Better Performance

pg_tviews works without any additional extensions, but installing `jsonb_delta` provides **1.5-3× faster** JSONB updates:

### Performance Impact

| Operation | Without jsonb_delta | With jsonb_delta | Improvement |
|-----------|-------------------|----------------|-------------|
| JSONB field updates | Full document replacement | Surgical patching | 1.5-3× faster |
| Memory usage | Higher | Lower | 30-50% reduction |
| CPU usage | Higher | Lower | 40-60% reduction |

### Installation

```sql
-- Install jsonb_delta extension
CREATE EXTENSION jsonb_delta;

-- Verify pg_tviews detects it
SELECT pg_tviews_check_jsonb_delta();  -- Should return true
```

### When You Need jsonb_delta

**Required for**:
- High-frequency JSONB updates (>100 ops/sec)
- Large JSONB objects (>100 fields)
- Performance-critical applications

**Optional for**:
- Read-heavy workloads
- Small JSONB objects (<20 fields)
- Infrequent updates (<10 ops/sec)

### Without jsonb_delta

pg_tviews still works perfectly - it just uses PostgreSQL's standard `jsonb_set()` function for updates, which replaces the entire JSONB document. This is slower but functionally identical.

## Platform-Specific Installation

### Ubuntu/Debian

```bash
# Install PostgreSQL (choose your version)
sudo apt-get update
sudo apt-get install postgresql-17 postgresql-server-dev-17

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Install pgrx and build
cargo install --locked cargo-pgrx
cargo pgrx init
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews
cargo pgrx install --release
```

### CentOS/RHEL/Rocky Linux

```bash
# Install PostgreSQL from PGDG repository
sudo yum install -y https://download.postgresql.org/pub/repos/yum/reporpms/EL-8-x86_64/pgdg-redhat-repo-latest.noarch.rpm
sudo yum install -y postgresql17-server postgresql17-devel

# Initialize and start PostgreSQL
sudo /usr/pgsql-17/bin/postgresql-17-setup initdb
sudo systemctl start postgresql-17

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Install pgrx and build
cargo install --locked cargo-pgrx
cargo pgrx init
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews
cargo pgrx install --release
```

### macOS (Homebrew)

```bash
# Install PostgreSQL
brew install postgresql@17
brew services start postgresql@17

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Install pgrx and build
cargo install --locked cargo-pgrx
cargo pgrx init
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews
cargo pgrx install --release
```

### Docker

```dockerfile
# Use PostgreSQL with pg_tviews pre-installed
FROM postgres:17

# Copy pg_tviews extension files
COPY --from=pgtviews-builder /usr/share/postgresql/17/extension/pg_tviews* /usr/share/postgresql/17/extension/
COPY --from=pgtviews-builder /usr/lib/postgresql/17/lib/pg_tviews.so /usr/lib/postgresql/17/lib/

# Enable extension in database initialization
COPY init.sql /docker-entrypoint-initdb.d/
```

```sql
-- init.sql
CREATE EXTENSION pg_tviews;
```

## Development Installation

For contributors and development:

### Clone and Setup

```bash
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews

# Install development dependencies
cargo install --locked cargo-pgrx cargo-watch cargo-expand

# Initialize pgrx
cargo pgrx init
```

### Development Build

```bash
# Debug build (slower but with debug symbols)
cargo pgrx install

# Release build (optimized)
cargo pgrx install --release

# Development with live reload
cargo watch -x 'pgrx install'
```

### Testing Setup

```bash
# Run tests
cargo pgrx test

# Run specific test
cargo pgrx test --package pg_tviews --lib

# Run integration tests
./run_red_tests.sh
```

## Production Deployment

### Multi-Server Setup

For production environments with multiple PostgreSQL servers:

```bash
# Build on a dedicated build server
cargo pgrx install --release --pg-config /path/to/pg_config

# Copy extension files to production servers
# - pg_tviews.so → $libdir/
# - pg_tviews.control → $sharedir/extension/
# - pg_tviews--*.sql → $sharedir/extension/
```

### Connection Pooling

pg_tviews is compatible with popular connection poolers:

#### PgBouncer

```ini
# pgbouncer.ini
[databases]
mydb = host=localhost port=5432 dbname=mydb

[pgbouncer]
pool_mode = transaction
server_reset_query = DISCARD ALL  # pg_tviews handles this automatically
```

#### pgpool-II

```ini
# pgpool.conf
server_reset_query = DISCARD ALL
```

### High Availability

pg_tviews works with PostgreSQL streaming replication and logical replication. Each server maintains its own TVIEWs and triggers.

## Verification

After installation, verify everything is working:

```sql
-- Check extension is installed
\dx pg_tviews

-- Check version
SELECT pg_tviews_version();

-- Run health check
SELECT * FROM pg_tviews_health_check();

-- Test basic functionality
CREATE TABLE test_table (id SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE test_tview AS SELECT id, data::jsonb as data FROM test_table;
INSERT INTO test_table (data) VALUES ('test');
SELECT * FROM test_tview;
```

## Troubleshooting Installation

### Build Errors

**pgrx not found:**
```bash
cargo install --locked cargo-pgrx
```

**PostgreSQL development headers missing:**
```bash
# Ubuntu/Debian
sudo apt-get install postgresql-server-dev-17

# CentOS/RHEL
sudo yum install postgresql17-devel
```

**Rust version too old:**
```bash
rustup update
```

### Runtime Errors

**Extension not found:**
```sql
-- Check if files are in correct locations
SHOW shared_preload_libraries;
\getenv sharedir
\getenv libdir
```

**Permission denied:**
```sql
-- Check database user permissions
SELECT current_user;
-- May need to run as superuser or grant permissions
```

**Library not loaded:**
```sql
-- Check PostgreSQL logs for dynamic loading errors
-- Verify pg_tviews.so is in $libdir and has correct permissions
```

## Updating pg_tviews

To update to a new version:

```bash
# Stop PostgreSQL
sudo systemctl stop postgresql

# Update source
cd pg_tviews
git pull
cargo pgrx install --release

# Start PostgreSQL
sudo systemctl start postgresql

# Update extension in database
ALTER EXTENSION pg_tviews UPDATE;
```

## Uninstallation

To remove pg_tviews:

```sql
-- Drop all TVIEWs first
DROP TABLE tv_my_view;

-- Drop extension
DROP EXTENSION pg_tviews;

-- Remove files (if installed manually)
rm /usr/lib/postgresql/17/lib/pg_tviews.so
rm /usr/share/postgresql/17/extension/pg_tviews*
```

## Next Steps

- **[Quick Start](quickstart.md)** - Create your first TVIEW
- **[FraiseQL Integration](fraiseql-integration.md)** - Framework integration patterns
- **[Monitoring](../operations/monitoring.md)** - Production monitoring setup
# pg_tviews

<div align="center">

**Transactional Materialized Views with Incremental Refresh for PostgreSQL**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![PostgreSQL](https://img.shields.io/badge/PostgreSQL-16-blue.svg)](https://www.postgresql.org/)
[![Rust](https://img.shields.io/badge/Rust-1.81%2B-orange.svg)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-0.1.0--beta.11-orange.svg)](https://github.com/fraiseql/pg_tviews/releases)
[![Status](https://img.shields.io/badge/status-beta-blue.svg)](https://github.com/fraiseql/pg_tviews/releases)

**CI/CD Status**:
[![CI](https://github.com/fraiseql/pg_tviews/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/ci.yml)
[![Clippy Strict](https://github.com/fraiseql/pg_tviews/actions/workflows/clippy.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/clippy.yml)
[![Integration Test](https://github.com/fraiseql/pg_tviews/actions/workflows/coverage.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/coverage.yml)
[![Security Audit](https://github.com/fraiseql/pg_tviews/actions/workflows/security-audit.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/security-audit.yml)
[![Documentation](https://github.com/fraiseql/pg_tviews/actions/workflows/docs.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/docs.yml)

*Core infrastructure for FraiseQL's GraphQL Cascade — automatic incremental refresh of JSONB read models with 5,000-12,000× performance gains.*

By Lionel Hamayon • Part of the FraiseQL framework

[Features](#-key-features) •
[Quick Start](#-quick-start) •
[Performance](#-performance) •
[Documentation](#-documentation) •
[Architecture](#-architecture)

</div>

---

## 🍓 Part of the FraiseQL Ecosystem

**pg_tviews** is the performance foundation for FraiseQL's CQRS architecture:

### **Server Stack (PostgreSQL + Python/Rust)**

| Tool | Purpose | Status | Performance Gain |
|------|---------|--------|------------------|
| **[pg_tviews](https://github.com/fraiseql/pg_tviews)** | Incremental materialized views | **Beta** ⭐ | **100-500× faster** |
| **[jsonb_delta](https://github.com/evoludigit/jsonb_delta)** | JSONB surgical updates | Stable | **2-7× faster** |
| **[pgGit](https://pggit.dev)** | Database version control | Stable | Git for databases |
| **[confiture](https://github.com/fraiseql/confiture)** | PostgreSQL migrations | Stable | **300-600× faster** |
| **[fraiseql](https://fraiseql.dev)** | GraphQL framework | Stable | **7-10× faster** |
| **[fraiseql-data](https://github.com/fraiseql/fraiseql-seed)** | Seed data generation | Planned | Auto-dependency resolution |

### **Client Libraries (TypeScript/JavaScript)**

| Library | Purpose | Framework Support |
|---------|---------|-------------------|
| **[graphql-cascade](https://github.com/graphql-cascade/graphql-cascade)** | Automatic cache invalidation | Apollo, React Query, Relay, URQL |

**How pg_tviews fits:**
- **fraiseql** uses pg_tviews for GraphQL read models (tv_* tables)
- **jsonb_delta** optimizes JSONB updates (1.5-3× faster)
- **confiture** manages TVIEW schema evolution
- **graphql-cascade** (client-side) invalidates browser caches when mutations trigger refreshes

**Stack it up:**
```bash
# Install extensions
CREATE EXTENSION pg_tviews;
CREATE EXTENSION jsonb_delta;  -- Optional: 1.5-3× faster JSONB

# Create incremental view
CREATE TABLE tv_post AS SELECT ...;

# Use with fraiseql GraphQL
@fraiseql.type(sql_source="tv_post")
class Post: ...
```

---

## 📋 Version Status

**Current Version**: `0.1.0-beta.11` (April 2026)
- **Status**: Public Beta - Feature-complete, API may change
- **Production Use**: Suitable for evaluation, not mission-critical systems
- **Support**: Community support via GitHub issues

**Roadmap to 1.0.0**:
- ✅ Core TVIEW functionality complete
- ✅ Comprehensive documentation
- 🔄 Production hardening and testing
- 🔄 Security audit
- 🔄 Performance validation at scale

**Breaking Changes**: Minor API changes possible until 1.0.0. Pin to exact version in production.

---

## 🎯 The Problem

Traditional PostgreSQL materialized views require full rebuilds on every refresh—scanning entire tables and recomputing all rows. For large datasets or complex views with JOINs, this becomes prohibitively expensive:

```sql
-- Traditional approach: Full rebuild every time
REFRESH MATERIALIZED VIEW my_view;  -- Scans ALL rows, recomputes EVERYTHING
```

**Result**: Minutes of downtime, high I/O, locks, and stale data between refreshes.

## ✨ The Solution

**pg_tviews** brings **incremental materialized view maintenance** to PostgreSQL with surgical, row-level updates that happen automatically within your transactions:

```sql
-- pg_tviews: Automatic incremental refresh
CREATE TABLE tv_post AS
SELECT p.pk_post as pk_post, jsonb_build_object(...) as data
FROM tb_post p JOIN tb_user u ON p.fk_user = u.pk_user;

-- Just use your database normally:
INSERT INTO tb_post(title, fk_user) VALUES ('New Post', 123);
COMMIT;  -- tv_post automatically updated with ONLY the affected row!
```

**Result**: Millisecond updates, no full scans, always up-to-date, zero manual intervention.

### 🚀 Performance Optimization

For **1.5-3× faster JSONB updates**, install the optional `jsonb_delta` extension:

```sql
CREATE EXTENSION jsonb_delta;  -- Optional: 1.5-3× faster JSONB updates
CREATE EXTENSION pg_tviews;
```

Without `jsonb_delta`, pg_tviews uses standard PostgreSQL JSONB operations (still fast, just not optimized).

---

## 🔑 Trinity Identifier Pattern

pg_tviews follows FraiseQL's trinity identifier conventions for optimal GraphQL Cascade performance:

- `id` (UUID): Public identifier for GraphQL/REST APIs
- `pk_entity` (integer): Primary key for efficient joins and lineage tracking
- `fk_*` (integer): Foreign keys for cascade propagation
- `identifier` (text): Optional unique slugs for SEO-friendly URLs
- `{parent}_id` (UUID): Optional UUID FKs for FraiseQL filtering

Example TVIEW with full trinity support:
```sql
CREATE TABLE tv_post AS
SELECT
    p.pk_post,           -- lineage root
    p.id,                -- GraphQL ID
    p.identifier,        -- SEO slug
    p.fk_user,           -- cascade FK
    u.id as user_id,     -- FraiseQL filtering FK
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'author', jsonb_build_object(
            'id', u.id,
            'identifier', u.identifier,
            'name', u.name,
            'email', u.email
        )
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

---

## 🚀 Key Features

### Automatic & Intelligent

- **🔍 Smart Dependency Detection**: Automatically analyzes SQL to find source tables and relationships
- **🎯 Surgical Updates**: Updates only affected rows—never full table scans
- **🔄 Transactional Consistency**: Refresh happens atomically within your transaction
- **📊 Cascade Propagation**: Automatically handles multi-level view dependencies

### High Performance

- **⚡ 100-500× Faster Triggers**: Statement-level triggers for bulk operations
- **💾 Query Plan Caching**: 10× faster with cached prepared statements
- **📦 Bulk Optimization**: N rows with just 2 queries instead of N queries
- **🎨 Smart Patching**: 2× performance boost with optional jsonb_delta integration
- **🚀 UNLOGGED Tables**: 2-3× write performance with automatic crash recovery

### Production-Ready

- **🏊 Connection Pooling**: Full PgBouncer/pgpool-II compatibility with DISCARD ALL handling
- **📈 Comprehensive Monitoring**: Real-time metrics, health checks, performance views
- **🛡️ Enterprise-Grade Code**: 100% clippy-strict compliance, panic-safe FFI, zero unwraps

### Compliance & Security

- **📋 SBOM Generation**: Automated Software Bill of Materials in SPDX 2.3 and CycloneDX 1.5 formats
- **🔐 Cryptographic Signing**: Sigstore keyless + GPG maintainer signatures for all releases
- **🛡️ Dependency Security**: Automated vulnerability scanning with cargo-audit + cargo-vet audits
- **🔄 Automated Updates**: Dependabot integration for security patches and updates
- **🏗️ Reproducible Builds**: Docker-based build environment with locked dependencies
- **🌍 International Compliance**: EU Cyber Resilience Act, US EO 14028, PCI-DSS 4.0, ISO 27001
- **🔒 Supply Chain Security**: SLSA Level 3 provenance with dependency transparency
- **📊 Vulnerability Management**: Complete dependency inventory for CVE tracking

### Developer-Friendly

- **📝 Simple API**: `pg_tviews_create()` function for easy TVIEW creation
- **🔧 JSONB Optimized**: Built for modern JSONB-heavy applications
- **📊 Array Support**: Full INSERT/DELETE handling for array columns
- **🐛 Excellent Debugging**: Rich error messages, debug functions, health checks
- **⏸️ Bulk Operations**: Suspend/resume triggers for safe bulk data loading (Issue #44)

---

## 📊 Performance

### Real-World Benchmarks

| Operation | Traditional MV | pg_tviews | Improvement |
|-----------|----------------|-----------|-------------|
| Single row update | 2,500ms | 1.2ms | 2,083× |
| Medium cascade (50 rows) | 7,550ms | 3.72ms | 2,028× |
| Bulk operation (1K rows) | 180,000ms | 100ms | 1,800× |

### Scaling Characteristics

- **Linear scaling** with data size for incremental updates
- **Sub-linear scaling** for cascading updates (graph caching)
- **Constant time** for cache hits (90%+ hit rate in production)
- **O(1) queue operations** with HashSet-based deduplication

---

## 🚀 UNLOGGED Tables

**pg_tviews** automatically creates TVIEWs as **UNLOGGED tables** for maximum write performance.

### Benefits

- **⚡ 2-3× Faster Writes**: No WAL overhead for TVIEW updates
- **🔄 Automatic Recovery**: Transparent crash recovery from base tables
- **💾 I/O Reduction**: Less disk writes for high-frequency updates
- **🔧 Configurable**: GUC parameter controls default behavior

### Crash Recovery

UNLOGGED tables are truncated on PostgreSQL crash, but **pg_tviews** automatically recovers:

```sql
-- Check and recover after potential crash
SELECT pg_tviews_recover_after_crash('user_summary');

-- Returns true if recovery was performed, false if not needed
```

### Configuration

All limits and toggles are runtime-tunable GUCs (`SET` per-session or set in
`postgresql.conf`); none require recompiling:

| GUC | Type | Default | Purpose |
|-----|------|---------|---------|
| `pg_tviews.max_propagation_depth` | int | 100 | Max cascade iterations before aborting |
| `pg_tviews.max_dependency_depth` | int | 10 | Max `pg_depend` traversal depth |
| `pg_tviews.max_queue_size` | int | 10000 | Refresh-queue backpressure limit |
| `pg_tviews.batch_size` | int | 1000 | Max PKs per bulk-refresh statement (chunking) |
| `pg_tviews.cache_size` | int | 10000 | Max entries per in-memory metadata cache |
| `pg_tviews.graph_cache_enabled` | bool | on | Cache dependency graphs |
| `pg_tviews.table_cache_enabled` | bool | on | Cache table→entity mappings |
| `pg_tviews.metrics_enabled` | bool | off | Collect refresh metrics |
| `pg_tviews.audit_enabled` | bool | off | Audit logging (opt-in) |
| `pg_tviews.unlogged_by_default` | bool | on | Create TVIEW tables UNLOGGED |
| `pg_tviews.suspend_triggers` | bool | off | Suspend trigger-based refresh (bulk loads) |
| `pg_tviews.union_duplicate_policy` | string | error | `first` or `error` on duplicate UNION-ALL keys |
| `pg_tviews.log_level` | string | info | Logging verbosity |

```sql
-- Examples
SET pg_tviews.unlogged_by_default = true;   -- default UNLOGGED behavior
SET pg_tviews.batch_size = 5000;            -- larger bulk-refresh chunks
SET pg_tviews.cache_size = 50000;           -- bigger per-session caches

-- Alter existing TVIEWs
ALTER TABLE tv_my_view SET UNLOGGED;
ALTER TABLE tv_my_view SET LOGGED;  -- ⚠️ Truncates data
```

> GUCs require `shared_preload_libraries = 'pg_tviews'` (already needed for the
> extension) so they are registered at backend start.

### TVIEW naming convention

A TVIEW's derived entity must be refreshable: either a `tb_<entity>` base table
exists (its primary key is `pk_<entity>`), or the definition joins the base tables
it derives from so cascade paths can route changes to it. `pg_tviews_create`
rejects a definition that satisfies neither, since it could never refresh.

### Safety Guarantees

- **✅ Data Recovery**: All TVIEW data reconstructible from base tables
- **✅ Transparent**: Applications work unchanged
- **✅ Configurable**: Can disable UNLOGGED for specific use cases
- **✅ Tested**: Comprehensive crash simulation and recovery testing

---

## 🎬 Quick Start

### Installation

```bash
# Prerequisites
# - PostgreSQL 16 installed
# - Rust toolchain 1.81+

# Install pgrx (must match project version)
cargo install --locked cargo-pgrx --version 0.16.1

# Initialize pgrx
cargo pgrx init

# Clone and build
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews
cargo pgrx install --release

# Enable in your database
psql -d your_database -c "CREATE EXTENSION pg_tviews;"
```

### Your First TVIEW

```sql
-- Create base tables (FraiseQL style)
CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    identifier TEXT UNIQUE,
    name TEXT,
    email TEXT
);

CREATE TABLE tb_post (
    pk_post BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    identifier TEXT UNIQUE,
    title TEXT,
    content TEXT,
    fk_user BIGINT REFERENCES tb_user(pk_user)
);

-- Create a TVIEW (note: tv_ prefix is required)
CREATE TABLE tv_post AS
SELECT
    p.pk_post as pk_post,  -- Primary key column (required)
    p.id,                  -- GraphQL ID
    p.identifier,          -- SEO slug
    p.fk_user,             -- Cascade FK
    u.id as user_id,       -- FraiseQL filtering FK
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'author', jsonb_build_object(
            'id', u.id,
            'identifier', u.identifier,
            'name', u.name,
            'email', u.email
        )
    ) as data  -- JSONB data column (required)
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;

-- Use it like a table
SELECT data FROM tv_post WHERE data->>'title' ILIKE '%rust%';

-- It updates automatically!
INSERT INTO tb_user (identifier, name, email) VALUES ('alice', 'Alice', 'alice@example.com');
INSERT INTO tb_post (identifier, title, content, fk_user) VALUES
    ('learning-rust', 'Learning Rust', 'Rust is amazing!', 1);

-- tv_post is now automatically up-to-date!
SELECT data FROM tv_post;
```

### Enable Advanced Features

```sql
-- Install statement-level triggers for 100-500× better bulk performance
SELECT pg_tviews_install_stmt_triggers();

-- Monitor system health
SELECT * FROM pg_tviews_health_check();

-- View real-time metrics
SELECT * FROM pg_tviews_queue_realtime;
```

---

## ⏸️ Bulk Operations (Issue #44)

For bulk INSERT/UPDATE/DELETE operations (e.g., seed data loading, ETL imports) on tables with TVIEWs, use the suspend/resume API to prevent trigger-based refresh during the operation:

### Basic Pattern

```sql
-- Suspend trigger-based refresh globally
SELECT pg_tviews_suspend_triggers();

-- Perform bulk operations
INSERT INTO customers SELECT * FROM staging_customers;
INSERT INTO orders SELECT * FROM staging_orders;

-- Resume triggers and refresh all TVIEWs in dependency order
SELECT pg_tviews_resume_triggers();
SELECT pg_tviews_refresh_all();
```

### Why This Matters

When bulk inserting into multiple related tables:
- Triggers fire on EACH insert
- But dependent TVIEWs may not have all their data yet
- This causes silent refresh failures (due to ON CONFLICT DO NOTHING)

The suspend/resume API ensures:
1. All data is loaded before any refresh happens
2. TVIEWs are refreshed in dependency order
3. JOINs in TVIEWs succeed because all tables are populated

### Transaction-Scoped

Suspension auto-resumes at transaction end:

```sql
BEGIN;
  SELECT pg_tviews_suspend_triggers();
  -- Bulk operations
  -- No explicit resume needed!
COMMIT;  -- Auto-resumes and enqueues changes

SELECT pg_tviews_refresh_all();
```

### API Reference

- `pg_tviews_suspend_triggers()` - Start suspension (supports nesting)
- `pg_tviews_resume_triggers()` - Resume; enqueues changed entities
- `pg_tviews_refresh_all()` - Refresh all queued TVIEWs in dependency order
- `pg_tviews_is_suspended()` - Check current suspension state
- `pg_tviews_suspended_entities()` - List entities that changed during suspension

### Nested Suspension

Calls can be nested; each must be matched:

```sql
SELECT pg_tviews_suspend_triggers();  -- depth 1
SELECT pg_tviews_suspend_triggers();  -- depth 2
-- operations...
SELECT pg_tviews_resume_triggers();   -- depth 1
SELECT pg_tviews_resume_triggers();   -- depth 0 (now resumed)
```

### Use Cases

- **Seed data loading**: DB initialization with initial data set
- **ETL imports**: Loading data from external sources into staging tables
- **Snapshot imports**: Restoring from database dumps or migrations
- **Bulk migrations**: Large data transformations

---

## 🏗️ Architecture

### High-Level Design

```
┌─────────────────────────────────────────────────────────────────┐
│                     User Application                            │
└────────────────────┬────────────────────────────────────────────┘
                     │ INSERT/UPDATE/DELETE
                     ▼
┌─────────────────────────────────────────────────────────────────┐
│                    PostgreSQL Core                              │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │  tb_* Tables │────▶│   Triggers   │────▶│ Refresh Queue│    │
│  │  (command)   │     │  (per-row or │     │ (thread-local)│   │
│  └──────────────┘     │  statement)  │     └──────┬────────┘    │
│                       └──────────────┘            │             │
│                       ┌──────────────┐            │             │
│                       │  ProcessUtil │            │             │
│                       │  Hook (DDL)  │            │             │
│                       └──────────────┘            │             │
│                                                   │             │
│                       ┌───────────────────────────▼──────────┐  │
│                       │    Transaction Callback Handler      │  │
│                       │  (PRE_COMMIT, COMMIT, ABORT, 2PC)    │  │
│                       └──────────┬────────────────────────────┘  │
│                                  │                               │
│                                  ▼                               │
│               ┌──────────────────────────────────────────┐      │
│               │      pg_tviews Refresh Engine            │      │
│               │                                           │      │
│               │  ┌─────────────────────────────────────┐ │      │
│               │  │  Dependency Graph Resolution        │ │      │
│               │  │  (Topological Sort, Cycle Detect)   │ │      │
│               │  └───────────┬──────────────────────────┘ │      │
│               │              │                            │      │
│               │              ▼                            │      │
│               │  ┌─────────────────────────────────────┐ │      │
│               │  │   Bulk Refresh Processor            │ │      │
│               │  │   (2 queries for N rows)            │ │      │
│               │  └───────────┬──────────────────────────┘ │      │
│               │              │                            │      │
│               │              ▼                            │      │
│               │  ┌─────────────────────────────────────┐ │      │
│               │  │  Cache Layer (Graph, Table, Plan)   │ │      │
│               │  └───────────┬──────────────────────────┘ │      │
│               │              │                            │      │
│               │              ▼                            │      │
│               │  ┌─────────────────────────────────────┐ │      │
│               │  │    Metrics & Monitoring              │ │      │
│               │  └─────────────────────────────────────┘ │      │
│               └──────────────────────────────────────────┘      │
│                                  │                               │
│                                  ▼                               │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │  TVIEW Tables│◀────│  Backing     │◀────│   Metadata   │    │
│  │  (tv_*)      │     │  Views (v_*) │     │  (pg_tview_*)│    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

### Key Components

1. **Trigger System**: Captures changes at source tables, enqueues refresh operations
2. **Transaction Queue**: Thread-local HashSet for deduplication and ACID guarantees
3. **Dependency Graph**: Resolves refresh order, detects cycles, enables cascading
4. **Refresh Engine**: Executes surgical updates with bulk optimization
5. **Cache Layer**: Three-tier caching (graph, table OIDs, query plans)
6. **Monitoring**: Real-time metrics, health checks, performance analytics

---

## 📚 Documentation

### Getting Started
- **[Quick Start](docs/getting-started/quickstart.md)** - Step-by-step setup guide
- **[Installation](docs/getting-started/installation.md)** - Detailed installation instructions
- **[FraiseQL Integration](docs/getting-started/fraiseql-integration.md)** - Framework integration guide

### User Guides
- **[For Developers](docs/user-guides/developers.md)** - Application integration patterns
- **[For Operators](docs/user-guides/operators.md)** - Production deployment guide
- **[For Architects](docs/user-guides/architects.md)** - CQRS design decisions

### Reference
- **[API Reference](docs/reference/api.md)** - Complete function reference
- **[DDL Reference](docs/reference/ddl.md)** - CREATE/DROP TABLE syntax
- **[Syntax Comparison](docs/getting-started/syntax-comparison.md)** - TVIEW creation methods
- **[Error Reference](docs/reference/errors.md)** - Error types and solutions
- **[Configuration](docs/reference/configuration.md)** - Configuration options

### Operations
- **[Monitoring](docs/operations/monitoring.md)** - Metrics and health checks
- **[Troubleshooting](docs/operations/troubleshooting.md)** - Debugging procedures
- **[Performance](docs/operations/performance.md)** - 📊 Complete performance guide (index)
  - [Performance Best Practices](docs/operations/performance-best-practices.md) - Essential patterns
  - [Performance Analysis](docs/operations/performance-analysis.md) - Diagnostic tools
  - [Index Optimization](docs/operations/index-optimization.md) - Index strategies
  - [Performance Tuning](docs/operations/performance-tuning.md) - Advanced tuning
  - **[Security](docs/operations/security.md)** - Security best practices
  - **[SBOM](docs/security/sbom.md)** - Software Bill of Materials and supply chain security
- **[Disaster Recovery](docs/operations/disaster-recovery.md)** - Backup and recovery
- **[Runbooks](docs/operations/runbooks.md)** - Operational procedures
- **[Upgrades](docs/operations/upgrades.md)** - Version migration guides

### Benchmarks
- **[Overview](docs/benchmarks/overview.md)** - Methodology and the three-arm comparison
- **[Running Benchmarks](docs/benchmarks/running-benchmarks.md)** - How to run the harness on a local pgrx cluster
- **[Results](docs/benchmarks/results.md)** - Measured performance figures
- **[Results Interpretation](docs/benchmarks/results-interpretation.md)** - Reading the numbers honestly
- **[jsonb_delta Integration](docs/benchmarks/jsonb-ivm-integration.md)** - jsonb_delta's role and the parity finding

### Development
- **[Contributing](docs/development/contributing.md)** - Development setup and contribution guidelines
- **[Testing](docs/development/testing.md)** - Testing patterns and procedures
- **[Architecture Deep Dive](docs/development/architecture-deep-dive.md)** - Technical architecture details

---

## 🎯 Use Cases

### Perfect For:

✅ **FraiseQL Applications** - Real-time GraphQL Cascade with UUID filtering
✅ **E-commerce Dashboards** - Real-time product aggregations with inventory
✅ **Analytics Workloads** - Pre-aggregated reporting tables that stay fresh
✅ **API Response Caching** - JSONB views for fast API responses
✅ **Activity Feeds** - User timelines with JOINed data
✅ **Denormalization** - Read-optimized tables without manual cache invalidation

### Not Recommended For:

❌ **Write-Heavy Tables** - If you have >1000 writes/sec per table
❌ **Simple Queries** - If a regular index works fine
❌ **Append-Only Logs** - No need for incremental refresh

---

## 🤝 Contributing

Contributions welcome! This is a portfolio project, but I'm happy to collaborate:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

**Development Setup**: See [DEVELOPMENT.md](DEVELOPMENT.md)

---

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

<div align="center">

**⭐ If you find this project interesting, please consider starring it! ⭐**

*Built with ❤️ and Rust 🦀*

</div>
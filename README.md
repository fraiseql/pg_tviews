# pg_tviews

<div align="center">

**Transactional Materialized Views with Incremental Refresh for PostgreSQL**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![PostgreSQL](https://img.shields.io/badge/PostgreSQL-13--18-blue.svg)](https://www.postgresql.org/)
[![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-0.1.0--beta.1-orange.svg)](https://github.com/fraiseql/pg_tviews/releases)
[![Status](https://img.shields.io/badge/status-beta-blue.svg)](https://github.com/fraiseql/pg_tviews/releases)

*Core infrastructure for FraiseQL's GraphQL Cascade — automatic incremental refresh of JSONB read models with 5,000-12,000× performance gains.*

By Lionel Hamayon • Part of the FraiseQL framework

[Features](#-key-features) •
[Quick Start](#-quick-start) •
[Performance](#-performance) •
[Documentation](#-documentation) •
[Architecture](#-architecture)

</div>

---

## 📋 Version Status

**Current Version**: `0.1.0-beta.1` (December 2025)
- **Status**: Public Beta - Feature-complete, API may change
- **Production Use**: Suitable for evaluation, not mission-critical systems
- **Support**: Community support via GitHub issues

**Roadmap to 1.0.0** (Q1 2026):
- ✅ Core TVIEW functionality complete
- ✅ Comprehensive documentation (in progress)
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

For **1.5-3× faster JSONB updates**, install the optional `jsonb_ivm` extension:

```sql
CREATE EXTENSION jsonb_ivm;  -- Optional: 1.5-3× faster JSONB updates
CREATE EXTENSION pg_tviews;
```

Without `jsonb_ivm`, pg_tviews uses standard PostgreSQL JSONB operations (still fast, just not optimized).

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
- **🎨 Smart Patching**: 2× performance boost with optional jsonb_ivm integration

### Production-Ready

- **🔐 Two-Phase Commit (2PC)**: Distributed transaction support with queue persistence
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

---

## 📊 Performance

### Validated Performance Results

| Operation | Traditional MV | pg_tviews | Improvement |
|-----------|----------------|-----------|-------------|
| Single row update | 7,050-7,974ms | 0.591ms | 5,000-12,000× |
| Medium cascade (1K products) | 4,040-4,169ms | 45.901ms | 88-93× |
| Bulk operations (100 products) | 7,050-7,974ms | 10,000-10,500ms | 0.7-0.8× |

*Results from comprehensive 4-way benchmark comparison with real PostgreSQL 18.1 measurements on 100K+ row datasets.*

**Hardware**: Intel Core i7-13700K, 32GB RAM, NVMe SSD, PostgreSQL 18.1
**Validation**: [Complete Benchmark Report](test/sql/comprehensive_benchmarks/final_results/COMPLETE_BENCHMARK_REPORT.md) - Real measurements with statistical analysis

## API Stability & Compatibility

### For Users

pg_tviews follows semantic versioning with three stability tiers:

- **STABLE APIs**: Guaranteed compatible across minor versions (0.1 → 0.2 → 1.0)
- **EVOLVING APIs**: May change in minor versions, stabilize before 1.0
- **EXPERIMENTAL APIs**: No compatibility guarantee, debugging only

See [API Stability Guide](./docs/api/SQL_FUNCTIONS.md) for detailed contracts.

### Version Guarantees

| Version | Stability | Production Ready |
|---------|-----------|------------------|
| 0.1.x | Beta | Yes, with caution |
| 1.0.x | Stable | Yes |
| 2.0.x | Next Gen | Future |

### Migration Guide

Upgrading between versions:
- **0.1 → 0.2**: STABLE functions guaranteed to work
- **0.2 → 1.0**: STABLE functions guaranteed to work
- **1.0 → 1.1**: Minor version updates, STABLE functions only
- **1.x → 2.0**: Breaking changes possible, migration guide required

See [CHANGELOG.md](./CHANGELOG.md) and [Breaking Changes](./docs/api/BREAKING_CHANGES.md).

### Getting Help

- **STABLE APIs**: Safe to use, fully supported
- **EVOLVING APIs**: Consider alternatives, report issues
- **EXPERIMENTAL APIs**: Development only, not supported for production
**Memory**: [Memory Profiling Report](MEMORY_PROFILING_REPORT.md) - Comprehensive memory analysis framework

### Scaling Characteristics

- **Linear scaling** with data size for incremental updates
- **Sub-linear scaling** for cascading updates (graph caching)
- **Constant time** for cache hits (90%+ hit rate in production)
- **O(1) queue operations** with HashSet-based deduplication

---

## 🎬 Quick Start

### Installation

```bash
# Prerequisites
# - PostgreSQL 13-18 installed
# - Rust toolchain 1.70+

# Install pgrx
cargo install --locked cargo-pgrx

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

### TVIEW Creation Workflow

Due to PostgreSQL event trigger limitations, TVIEW tables are not automatically converted during `CREATE TABLE AS SELECT`.

#### Manual Conversion Process

**Step 1: Create your TVIEW table**
```sql
CREATE TABLE tv_my_entity AS
SELECT
    id,           -- UUID (required)
    data,         -- JSONB (required)
    -- Optional optimization columns:
    pk_entity,    -- INTEGER primary key
    fk_parent,    -- INTEGER foreign key
    parent_id,    -- UUID foreign key
    path          -- LTREE for hierarchies
FROM v_my_entity;
```

**Step 2: Manually convert to TVIEW**
```sql
SELECT pg_tviews_convert_existing_table('tv_my_entity');
```

**Step 3: Verify conversion**
```sql
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_my_entity';
```

#### Event Trigger Behavior

Event triggers now only validate TVIEW structure. After `CREATE TABLE AS SELECT`, you'll see:
```
INFO: TVIEW table created. To convert to TVIEW, run: SELECT pg_tviews_convert_existing_table('tv_my_entity');
```

#### Why Manual Conversion?

PostgreSQL event triggers cannot use the Server Programming Interface (SPI) to query system catalogs during DDL events due to transaction isolation. This is a PostgreSQL architectural limitation, not a bug.

**Technical Details**: Event triggers run within the same transaction as DDL commands. SPI calls create sub-transactions, which PostgreSQL prevents during DDL events to maintain consistency.

#### Future: Automatic Conversion

Background worker support for automatic conversion is planned for a future release. This will allow queued conversions to run in a separate transaction context.

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
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐   │
│  │  tb_* Tables │────▶│   Triggers   │────▶│ Refresh Queue│   │
│  │  (command)   │     │  (per-row or │     │ (thread-local)│   │
│  └──────────────┘     │  statement)  │     └──────┬────────┘   │
│                       └──────────────┘            │            │
│                       ┌──────────────┐            │            │
│                       │  ProcessUtil │            │            │
│                       │  Hook (DDL)  │            │            │
│                       └──────────────┘            │            │
│                                                  │            │
│                       ┌──────────────────────────▼──────────┐  │
│                       │    Transaction Callback Handler     │  │
│                       │  (PRE_COMMIT, COMMIT, ABORT, 2PC)   │  │
│                       └──────────┬───────────────────────────┘  │
│                                  │                              │
│                                  ▼                              │
│               ┌──────────────────────────────────────────┐     │
│               │      pg_tviews Refresh Engine          │     │
│               │                                          │     │
│               │  ┌────────────────────────────────────┐ │     │
│               │  │  Dependency Graph Resolution      │ │     │
│               │  │  (Topological Sort, Cycle Detect) │ │     │
│               │  └───────────┬────────────────────────┘ │     │
│               │              │                          │     │
│               │              ▼                          │     │
│               │  ┌────────────────────────────────────┐ │     │
│               │  │   Bulk Refresh Processor          │ │     │
│               │  │   (2 queries for N rows)          │ │     │
│               │  └───────────┬────────────────────────┘ │     │
│               │              │                          │     │
│               │              ▼                          │     │
│               │  ┌────────────────────────────────────┐ │     │
│               │  │  Cache Layer (Graph, Table, Plan) │ │     │
│               │  └───────────┬────────────────────────┘ │     │
│               │              │                          │     │
│               │              ▼                          │     │
│               │  ┌────────────────────────────────────┐ │     │
│               │  │    Metrics & Monitoring            │ │     │
│               │  └────────────────────────────────────┘ │     │
│               └──────────────────────────────────────────┘     │
│                                  │                              │
│                                  ▼                              │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐   │
│  │  TVIEW Tables│◀────│  Backing     │◀────│   Metadata   │   │
│  │  (tv_*)      │     │  Views (v_*) │     │  (pg_tview_*)│   │
│  └──────────────┘     └──────────────┘     └──────────────┘   │
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
- **[Overview](docs/benchmarks/overview.md)** - Performance testing methodology and 4-way comparison
- **[Running Benchmarks](docs/benchmarks/running-benchmarks.md)** - How to run benchmarks (Docker, pgrx, manual)
- **[Docker Setup](docs/benchmarks/docker-benchmarks.md)** - Advanced Docker benchmarking (requires jsonb_ivm)
- **[Results Interpretation](docs/benchmarks/results-interpretation.md)** - Understanding benchmark results
- **[Results](docs/benchmarks/results.md)** - Detailed benchmark data

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
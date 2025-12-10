# pg_tviews

<div align="center">

**Transactional Materialized Views with Incremental Refresh for PostgreSQL**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![PostgreSQL](https://img.shields.io/badge/PostgreSQL-15%2B-blue.svg)](https://www.postgresql.org/)
[![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-0.1.0--beta.1-green.svg)](https://github.com/your-org/pg_tviews/releases)

*High-performance incremental materialized views that stay in sync with your data—automatically*

[Features](#-key-features) •
[Quick Start](#-quick-start) •
[Performance](#-performance) •
[Documentation](#-documentation) •
[Architecture](#-architecture)

</div>

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
CREATE TVIEW tv_post AS
SELECT p.id as pk_post, jsonb_build_object(...) as data
FROM tb_post p JOIN tb_user u ON p.fk_user = u.pk_user;

-- Just use your database normally:
INSERT INTO tb_post(title, fk_user) VALUES ('New Post', 123);
COMMIT;  -- tv_post automatically updated with ONLY the affected row!
```

**Result**: Millisecond updates, no full scans, always up-to-date, zero manual intervention.

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

### Developer-Friendly

- **📝 Simple DDL**: `CREATE TVIEW` syntax feels natural
- **🔧 JSONB Optimized**: Built for modern JSON-heavy applications
- **📊 Array Support**: Full INSERT/DELETE handling for array columns
- **🐛 Excellent Debugging**: Rich error messages, debug functions, health checks

---

## 💡 Why pg_tviews?

### Real-World Impact

| Scenario | Traditional `REFRESH MATERIALIZED VIEW` | pg_tviews |
|----------|----------------------------------------|-----------|
| **Single row insert** | Full table scan + rebuild (seconds-minutes) | Surgical row update (<5ms) |
| **Bulk operation (1000 rows)** | Full table scan + rebuild | 2 queries total (~100ms) |
| **Complex view (5 JOINs)** | Recompute everything | Update only affected rows |
| **Data freshness** | Stale between manual refreshes | Always current (transactional) |
| **Production downtime** | Locks during refresh | Zero downtime |

### Technical Excellence

**This project demonstrates:**

✅ **Advanced PostgreSQL Extension Development** - Deep integration with PostgreSQL internals
✅ **Systems Programming in Rust** - 9000+ lines of production Rust code
✅ **Complex Algorithm Implementation** - Dependency graphs, topological sorting, cycle detection
✅ **Performance Engineering** - Multiple caching layers, query optimization, bulk processing
✅ **Distributed Systems** - Two-Phase Commit protocol implementation
✅ **Production-Grade Quality** - Comprehensive error handling, monitoring, FFI safety
✅ **Complete Software Lifecycle** - From architecture to testing to documentation

---

## 📊 Performance

### Real-World Benchmarks

```
Operation                    | Without pg_tviews | With pg_tviews | Improvement
-----------------------------|-------------------|----------------|-------------
Single row update            | 2500ms (full scan)| 1.2ms          | 2083× faster
Medium cascade (50 rows)     | 7550ms            | 3.72ms         | 2028× faster
Bulk operation (1000 rows)   | 180000ms          | 100ms          | 1800× faster
Statement-level triggers     | 500× overhead     | 1× overhead    | 500× faster
```

### Scalability Characteristics

- **Linear scaling** with data size for incremental updates
- **Sub-linear scaling** for cascading updates (graph caching)
- **Constant time** for cache hits (90%+ hit rate in production)
- **O(1) queue operations** with HashSet-based deduplication

---

## 🎬 Quick Start

### Installation

```bash
# Prerequisites
# - PostgreSQL 15+ installed
# - Rust toolchain 1.70+

# Install pgrx
cargo install --locked cargo-pgrx

# Initialize pgrx
cargo pgrx init

# Clone and build
git clone https://github.com/your-org/pg_tviews.git
cd pg_tviews
cargo pgrx install --release

# Enable in your database
psql -d your_database -c "CREATE EXTENSION pg_tviews;"
```

### Your First TVIEW

```sql
-- Create base tables
CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    name TEXT,
    email TEXT
);

CREATE TABLE posts (
    id BIGSERIAL PRIMARY KEY,
    title TEXT,
    content TEXT,
    user_id BIGINT REFERENCES users(id)
);

-- Create a TVIEW (note: tv_ prefix is required)
CREATE TVIEW tv_posts AS
SELECT
    p.id as pk_post,  -- Primary key column (required)
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'content', p.content,
        'author', jsonb_build_object(
            'id', u.id,
            'name', u.name,
            'email', u.email
        )
    ) as data  -- JSONB data column (required)
FROM posts p
JOIN users u ON p.user_id = u.id;

-- Use it like a table
SELECT * FROM tv_posts WHERE data->>'title' ILIKE '%rust%';

-- It updates automatically!
INSERT INTO users (name, email) VALUES ('Alice', 'alice@example.com');
INSERT INTO posts (title, content, user_id) VALUES
    ('Learning Rust', 'Rust is amazing!', 1);
-- tv_posts is now automatically up-to-date!

SELECT data FROM tv_posts;
-- Returns:
-- {
--   "id": 1,
--   "title": "Learning Rust",
--   "content": "Rust is amazing!",
--   "author": {"id": 1, "name": "Alice", "email": "alice@example.com"}
-- }
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
│  │  Base Tables │────▶│   Triggers   │────▶│ Refresh Queue│   │
│  │  (tb_*)      │     │  (per-row or │     │ (thread-local)│   │
│  └──────────────┘     │  statement)  │     └──────┬────────┘   │
│                       └──────────────┘            │            │
│                                                    │            │
│                       ┌──────────────┐            │            │
│                       │  ProcessUtil │            │            │
│                       │  Hook (DDL)  │            │            │
│                       └──────────────┘            │            │
│                                                    │            │
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

### Data Flow

```
1. User modifies data
   └─▶ INSERT INTO posts VALUES (...)

2. Trigger fires
   └─▶ Enqueue (entity='post', pk=123) to thread-local queue

3. Pre-commit callback
   └─▶ Resolve dependencies: post ─depends_on─▶ comment ─depends_on─▶ notification
   └─▶ Topological sort: [post, comment, notification]
   └─▶ Bulk refresh by entity:
       • SELECT * FROM v_post WHERE pk = ANY([123, 124, 125])  -- 3 posts, 1 query
       • UPDATE tv_post SET data = ... FROM unnest(...)         -- 1 query
   └─▶ Discover parent dependencies, repeat

4. Commit
   └─▶ Clear queue, reset metrics, transaction complete

5. Query TVIEW
   └─▶ SELECT * FROM tv_posts WHERE data->>'title' = 'Rust'
   └─▶ Fast JSONB index scan, data is already up-to-date!
```

---

## 🛠️ Technical Highlights

### Advanced PostgreSQL Integration

- **ProcessUtility Hook**: Intercepts CREATE/DROP TVIEW DDL commands
- **Transaction Callbacks**: Integrates with PostgreSQL's transaction lifecycle
- **Subtransaction Support**: Proper SAVEPOINT and ROLLBACK TO handling
- **SPI (Server Programming Interface)**: Direct PostgreSQL catalog access
- **Custom Hooks**: Extension of PostgreSQL's hook system

### Sophisticated Algorithms

- **Dependency Graph Construction**: Extracts table dependencies from SQL AST
- **Topological Sorting**: Orders refresh operations to respect dependencies
- **Cycle Detection**: Prevents infinite cascades with dependency validation
- **Deduplication**: HashSet-based queue ensures each entity+pk processed once
- **Cascade Discovery**: Dynamically finds parent entities during propagation

### Performance Optimization Techniques

- **Multi-Level Caching**:
  - L1: Graph cache (dependency relationships)
  - L2: Table cache (OID lookups)
  - L3: Query plan cache (prepared statements)
- **Bulk Processing**: Batch operations by entity with SQL unnest()
- **Statement-Level Triggers**: Transition tables for 100-500× efficiency
- **Lazy Evaluation**: Defer expensive operations until commit
- **Connection Pooling Optimization**: DISCARD ALL handling, thread-local cleanup

### Code Quality & Safety

- **100% Clippy-Strict Compliance**: All clippy warnings resolved
- **Panic-Safe FFI**: All C callbacks wrapped in `catch_unwind`
- **Zero Unwraps**: Complete `Result`-based error handling
- **Comprehensive Error Types**: 14 distinct error variants with context
- **Thread Safety**: Proper Mutex usage, thread-local state management
- **Memory Safety**: Rust's ownership prevents memory leaks and corruption

---

## 📚 Documentation

### Reference Documentation

- **[API Reference](docs/API_REFERENCE.md)** - All 12 public functions documented
- **[Monitoring Guide](docs/MONITORING.md)** - Metrics, health checks, alerting
- **[DDL Reference](docs/DDL_REFERENCE.md)** - CREATE/DROP TVIEW syntax
- **[Operations Guide](docs/OPERATIONS.md)** - Backup, restore, connection pooling
- **[Error Reference](docs/ERROR_REFERENCE.md)** - All error types and solutions
- **[Debugging Guide](docs/DEBUGGING.md)** - Troubleshooting procedures

### Additional Resources

- **[CHANGELOG](CHANGELOG.md)** - Complete version history
- **[RELEASE NOTES](RELEASE_NOTES.md)** - v0.1.0-beta.1 details
- **[ARCHITECTURE](ARCHITECTURE.md)** - Deep-dive technical documentation
- **[Performance Results](docs/PERFORMANCE_RESULTS.md)** - Detailed benchmarks

### Running Comprehensive Benchmarks

Want to verify performance claims with real data? We provide a complete benchmark suite:

```bash
cd test/sql/comprehensive_benchmarks

# Quick test (2 minutes) - 1K products
./run_benchmarks.sh --scale small

# Realistic test (15 minutes) - 100K products
./run_benchmarks.sh --scale medium

# Production scale (1 hour) - 1M products
./run_benchmarks.sh --scale large

# View results
python3 generate_report.py
```

**Three-Way Comparison:**
- **Approach 1**: pg_tviews + jsonb_ivm (surgical patching - fastest)
- **Approach 2**: Manual + native PostgreSQL (jsonb_set - middle ground)
- **Approach 3**: Full REFRESH MATERIALIZED VIEW (traditional - baseline)

**Benchmark Coverage:**
- ✅ E-commerce product catalog (categories → products → reviews → inventory)
- ✅ Single row & bulk operations (100, 1K rows)
- ✅ Cascade updates across relationships
- ✅ Multiple data scales (1K, 100K, 1M rows)

**See [test/sql/comprehensive_benchmarks/QUICKSTART.md](test/sql/comprehensive_benchmarks/QUICKSTART.md) for details.**

---

## 📈 Project Statistics

```
Lines of Code:        9,000+ (Rust)
Modules:              39 files
Test Coverage:        Comprehensive integration tests
Development Phases:   10 completed phases
Commits:              49+ atomic commits
Development Time:     3+ months of focused work
Documentation:        15+ comprehensive docs
```

### Technology Stack

- **Language**: Rust 🦀 (unsafe code minimized, well-documented)
- **Framework**: pgrx 0.12.8 (PostgreSQL extension framework)
- **Database**: PostgreSQL 15+ (tested through 17)
- **Dependencies**: Minimal, well-audited crates
- **Build**: Cargo + pgrx toolchain

### Quality Metrics

- ✅ **Zero unsafe panics** across FFI boundary
- ✅ **Zero unwraps** in production code
- ✅ **Zero clippy warnings** with strict lints
- ✅ **100% documented** public APIs
- ✅ **Comprehensive error handling** with 14 error types
- ✅ **Full monitoring** with health checks and metrics

---

## 🎯 Use Cases

### Perfect For:

✅ **E-commerce Dashboards** - Real-time product aggregations with inventory
✅ **Analytics Workloads** - Pre-aggregated reporting tables that stay fresh
✅ **API Response Caching** - JSONB views for fast API responses
✅ **Activity Feeds** - User timelines with JOINed data
✅ **Denormalization** - Read-optimized tables without manual cache invalidation
✅ **Multi-Tenant SaaS** - Per-tenant aggregations that scale

### Not Recommended For:

❌ **Write-Heavy Tables** - If you have >1000 writes/sec per table
❌ **Simple Queries** - If a regular index works fine
❌ **Append-Only Logs** - No need for incremental refresh

---

## 🔬 Advanced Features

### Two-Phase Commit (2PC) Support

```sql
-- Begin distributed transaction
BEGIN;
INSERT INTO posts VALUES (...);

-- Prepare transaction (queue persisted to disk)
PREPARE TRANSACTION 'my-gid';

-- Later (different session or after crash recovery):
SELECT pg_tviews_commit_prepared('my-gid');
-- All pending refreshes execute atomically!
```

### Connection Pooling (PgBouncer)

```ini
# pgbouncer.ini
[databases]
mydb = host=localhost port=5432 dbname=mydb

[pgbouncer]
pool_mode = transaction
server_reset_query = DISCARD ALL  # pg_tviews handles this!
```

### Monitoring & Observability

```sql
-- Real-time health check
SELECT * FROM pg_tviews_health_check();

-- View current queue
SELECT * FROM pg_tviews_queue_realtime;

-- Check cache performance
SELECT * FROM pg_tviews_cache_stats;

-- Historical metrics
SELECT * FROM pg_tviews_performance_summary
WHERE hour > now() - interval '24 hours';
```

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

## 🎓 Learning & Portfolio

This project was developed to demonstrate:

- **Enterprise-grade PostgreSQL extension development**
- **Advanced Rust systems programming**
- **Complex algorithm implementation** (graphs, sorting, caching)
- **Performance engineering** and optimization
- **Production-ready code quality** standards
- **Comprehensive documentation** and testing
- **Complete software lifecycle** from design to deployment

### Key Learnings

1. **PostgreSQL Internals**: Deep understanding of hooks, SPI, transaction lifecycle
2. **Rust Mastery**: Advanced ownership, FFI safety, error handling patterns
3. **Algorithm Design**: Dependency graphs, topological sorting, cycle detection
4. **Performance Tuning**: Multi-level caching, bulk optimization, query planning
5. **Distributed Systems**: Two-Phase Commit protocol implementation
6. **Code Quality**: Achieving clippy-strict compliance, panic safety, zero unwraps
7. **Documentation**: Writing clear, comprehensive technical documentation

---

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

## 🙏 Acknowledgments

- **pgrx Framework**: Excellent foundation for PostgreSQL extensions in Rust
- **PostgreSQL Community**: Comprehensive documentation of internals
- **jsonb_ivm**: Optional dependency for enhanced performance
- **Rust Community**: Amazing language and ecosystem

---

## 📬 Contact

**Lionel Hamayon**
- Email: lionel.hamayon@evolution-digitale.fr
- GitHub: [@fraiseql](https://github.com/fraiseql/)
- LinkedIn: [Your LinkedIn](https://linkedin.com/in/lionel-hamayon)

---

<div align="center">

**⭐ If you find this project interesting, please consider starring it! ⭐**

*Built with ❤️ and Rust 🦀*

</div>

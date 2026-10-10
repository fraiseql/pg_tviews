# pg_tviews

<div align="center">

**Transactional Materialized Views with Incremental Refresh for PostgreSQL**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![PostgreSQL](https://img.shields.io/badge/PostgreSQL-16%E2%80%9318-blue.svg)](https://www.postgresql.org/)
[![Rust](https://img.shields.io/badge/Rust-1.98-orange.svg)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-0.1.0--beta.25-orange.svg)](https://github.com/fraiseql/pg_tviews/releases)
[![Status](https://img.shields.io/badge/status-beta-blue.svg)](https://github.com/fraiseql/pg_tviews/releases)

**CI/CD Status**:
[![CI](https://github.com/fraiseql/pg_tviews/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/ci.yml)
[![Clippy Strict](https://github.com/fraiseql/pg_tviews/actions/workflows/clippy.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/clippy.yml)
[![Coverage](https://github.com/fraiseql/pg_tviews/actions/workflows/coverage.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/coverage.yml)
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
| **[pgGit](https://github.com/evoludigit/pgGit)** | Database per branch or agent, attributed DDL history | In development (v2) | Not a schema source of truth; promotes into confiture |
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

**Current Version**: `0.1.0-beta.25` (October 2026)
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
`CREATE EXTENSION pg_tviews` says so once, with a WARNING; after that each backend notes it
once in the server log, and `pg_tviews_health_check()` reports it. Writes send no message.

---

## 🔑 Identifiers

A TVIEW's definition outputs `pk_<entity>` (the first such column names the
entity; with `DISTINCT ON`, its key names the rows), and usually `id` and `data`.
Nothing else is a naming rule: base tables, join columns and the column holding an
embedded TVIEW's key may have any names, because pg_tviews reads the definition's
query tree, not its spelling.

FraiseQL's trinity identifiers fit this naturally:

- `id` (UUID): public identifier for GraphQL/REST APIs
- `pk_<entity>` (integer): the row key, for joins and refreshes
- `identifier` (text): optional unique slug
- `{parent}_id` (UUID): optional UUID of a parent, for filtering

Example:
```sql
CREATE TABLE tv_post AS
SELECT
    p.pk_post,           -- lineage root
    p.id,                -- GraphQL ID
    p.identifier,        -- SEO slug
    p.fk_user,           -- the author's key
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
- **🛡️ Dependency Security**: Automated vulnerability and license checks with cargo-audit and cargo-deny
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

## ⚡ Direct-patch fast path (Issue #56)

For an eligible `UPDATE`, pg_tviews builds a JSONB patch **at trigger time from
`NEW`** and applies it straight to `tv_<entity>.data` with
`jsonb_smart_patch_scalar/_nested` — **skipping the backing-view recompute
entirely** (zero reads of the backing view). The same patch is *derived* for
parent tviews that embed the entity as a nested object, so a single-field change
with a wide fan-out (e.g. an author edited on 60 posts) propagates as a handful of
grouped patch UPDATEs with no view queries.

Anything outside the eligibility boundary silently falls back to the normal
recompute path, which stays the single source of truth — the fast path is a pure
optimization and its `data` output is byte-identical to a recompute.

### What qualifies

The fast path engages only when **every** condition holds (otherwise recompute):

- the change is a row-level `UPDATE` (INSERT/DELETE change membership);
- `jsonb_delta` is installed and `pg_tviews.direct_patch_enabled = on`;
- the entity is not `DISTINCT ON` and not a `UNION`;
- every changed column maps identity-style to a top-level `data` key
  (`jsonb_build_object('bio', bio)` — no expression, cast, or other relation);
- no changed column is a foreign key, the primary key, or a column the tview also
  projects **outside** `data`;
- the changed values' types are TEXT/VARCHAR, INT2/4/8, BOOL, UUID, JSONB, or NULL
  (anything else — float, numeric, timestamps, arrays — falls back);
- for a parent, its dependency on the child is `nested_object` with a path
  (array/scalar/UUID-fk parents recompute).

### Copied parent columns (Issue #120)

A tview that copies a column of a joined parent table into its `data`
(`jsonb_build_object('author_name', u.name)` over `JOIN tb_user u ON u.pk_user =
p.fk_user`) is written by **one statement** when that column changes:
`UPDATE tv_post … WHERE fk_user = <pk>`, instead of recomputing every post of that
user. Same rules as above, plus: the tview reads the parent through one join on its
own base table, projects that `fk_*` column, reads the parent table only once, and
uses the column nowhere else in its definition. Anything else recomputes the
children. Measured with `test/sql/real_benchmark/scalar_cascade_fanout.sh … scalar`:
1.8–2.7× faster per parent update from 10 to 10 000 children.

### Kill-switch and observability

```sql
SET pg_tviews.direct_patch_enabled = off;   -- force the recompute path everywhere
```

`pg_tviews_queue_stats()` exposes the counters (session-cumulative):

```sql
SELECT pg_tviews_queue_stats();
-- { … "direct_patch_captured": N, "direct_patches_applied": N,
--     "direct_patch_fallbacks": N, "view_recomputes": N }
```

An eligible update leaves `view_recomputes` unchanged and bumps
`direct_patches_applied` — proof it skipped the view.

### Upgrade note

The column→key map is extracted **when a TVIEW is registered**. TVIEWs registered by
an older release keep working on the recompute path until
`SELECT * FROM tviews.pg_tviews_reregister_all()` re-derives their metadata in place
(see [Upgrading](#upgrading)).

---

## 🚀 UNLOGGED Tables

**pg_tviews** automatically creates TVIEWs as **UNLOGGED tables** for maximum write performance.

### Benefits

- **⚡ 2-3× Faster Writes**: No WAL overhead for TVIEW updates
- **🔄 Automatic Recovery**: Transparent crash recovery from base tables
- **💾 I/O Reduction**: Less disk writes for high-frequency updates
- **🔧 Configurable**: GUC parameter controls default behavior

### ⚠️ Hot standbys, promotion and crash restarts

An UNLOGGED table is not replicated. **A hot standby cannot read an UNLOGGED
TVIEW at all** (`ERROR: cannot access temporary or unlogged relations during
recovery`), and promotion or a crash restart leaves it empty. If reads are
routed to replicas, make those TVIEWs LOGGED:

```sql
SET pg_tviews.unlogged_by_default = off;              -- for new TVIEWs
SELECT pg_tviews_set_logged('post', true);            -- existing TVIEW (rewrites it)
SELECT * FROM pg_tviews_replication_status();         -- what a standby can serve
```

To repopulate emptied UNLOGGED TVIEWs as soon as a server leaves recovery, list
the databases in `pg_tviews.auto_rebuild_databases` (needs a restart), or call
`SELECT * FROM pg_tviews_rebuild_all();` after a failover or restore. See
[docs/operations/replication.md](docs/operations/replication.md).

### Crash Recovery

UNLOGGED tables are truncated on PostgreSQL crash. **pg_tviews** rebuilds a
TVIEW on the first write that touches it, and the startup worker above rebuilds
the configured databases without waiting for a write. To check one TVIEW by hand:

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
| `pg_tviews.audit_enabled` | bool | off | Audit logging (opt-in) |
| `pg_tviews.unlogged_by_default` | bool | on | Create TVIEW tables UNLOGGED (not readable on standbys) |
| `pg_tviews.auto_rebuild_databases` | string | "" | Databases whose emptied UNLOGGED TVIEWs are rebuilt when recovery ends (restart required) |
| `pg_tviews.data_gin_index` | bool | off | Create a GIN index on `data` for new TVIEWs |
| `pg_tviews.fillfactor` | int | 85 | Heap fillfactor for new TVIEW tables (keeps refreshes HOT) |
| `pg_tviews.direct_patch_enabled` | bool | on | Direct-patch fast path (see above) |
| `pg_tviews.suspend_triggers` | bool | off | Suspend trigger-based refresh (bulk loads) |
| `pg_tviews.union_duplicate_policy` | string | error | `first` or `error` on duplicate UNION-ALL keys |
| `pg_tviews.report_max_tracked` | int | 10000 | Changed rows journaled per transaction for `pg_tviews_flush_and_report()` (0 = off) |
| `pg_tviews.uncascaded_policy` | enum | error | `error`, `full_refresh` or `warn`: what a new TVIEW does about base tables no cascade reaches, when it declares no `uncascaded_policy` option. Read at create time and stored with the TVIEW; `error` refuses it, `full_refresh` recomputes the whole TVIEW on each write to such a table ([details](docs/reference/ddl.md#tables-no-cascade-reaches)) |
| `pg_tviews.time_refresh` | enum | none | `none` or `external`: whether a new TVIEW whose definition reads the current time (`CURRENT_DATE`, `now()`…) is accepted, when it declares no `time_refresh` option; `external` means `tviews.pg_tviews_refresh_time_dependent()` is called at the boundary ([details](docs/reference/ddl.md#time-dependent-tviews)) |
| `pg_tviews.log_level` | string | info | Logging verbosity |

```sql
-- Examples
SET pg_tviews.unlogged_by_default = true;   -- default UNLOGGED behavior
SET pg_tviews.batch_size = 5000;            -- larger bulk-refresh chunks
SET pg_tviews.cache_size = 50000;           -- bigger per-session caches

-- Alter existing TVIEWs (each rewrites the table under an exclusive lock)
SELECT pg_tviews_set_logged('my_view', false);  -- UNLOGGED
SELECT pg_tviews_set_logged('my_view', true);   -- LOGGED, readable on standbys
```

> GUCs require `shared_preload_libraries = 'pg_tviews'` (already needed for the
> extension) so they are registered at backend start.

### What a definition needs

A TVIEW's definition is one `SELECT` that outputs `pk_<entity>` and reads at least
one table. Every table it reads either maps its writes to the TVIEW's keys or
follows the TVIEW's `uncascaded_policy`: under the default, `error`, a definition
reading a table whose writes cannot be traced is refused at creation, with the
reason and how to declare a full refresh instead.

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
# - PostgreSQL 16, 17 or 18 (the supported versions; CREATE EXTENSION refuses older ones)
# - Rust toolchain (pinned in rust-toolchain.toml)

# Install pgrx (must match project version)
cargo install --locked cargo-pgrx --version 0.17.0

# Initialize pgrx
cargo pgrx init

# Clone and build
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews
cargo pgrx install --release   # PostgreSQL 18; for 16 or 17 add
                               # --no-default-features --features pg16 (or pg17)

# Enable in your database
psql -d your_database -c "CREATE EXTENSION pg_tviews;"
```

#### The `tviews` schema

Every object of the extension lives in the schema `tviews`, whatever the `search_path`
at `CREATE EXTENSION` (`CREATE EXTENSION pg_tviews SCHEMA other` is refused).
`CREATE EXTENSION` creates the schema when it is missing and refuses one owned by
another role. Call the functions qualified, or add `tviews` to the `search_path`:

```sql
SELECT tviews.pg_tviews_create('tv_post', $$ SELECT ... $$);

ALTER DATABASE your_database SET search_path = "$user", public, tviews;
SELECT pg_tviews_create('tv_post', $$ SELECT ... $$);
```

TVIEWs themselves (`tv_*`, `v_*`) are created in the schema you name, or in
`current_schema()`. Nothing pg_tviews does at run time needs `tviews` on the
`search_path`. `DROP EXTENSION pg_tviews` leaves the `tviews` schema behind, empty.

#### Privileges

A TVIEW is maintained by its owner, like a materialized view: when a role writes to a
base table, the refresh reads and writes each affected TVIEW as the owner of its
`tv_*` table, with `search_path` set to `pg_catalog, pg_temp`. Writers need only
their privileges on the base tables; grant application roles `SELECT` on the
`tv_*` tables they read. The owner needs `SELECT` on everything its definition
reads.

#### Creating TVIEWs from migrations

`tviews.pg_tviews_create_or_replace(name, query, options)` creates a TVIEW, or brings an
existing one to `query` and `options` with the smallest change, so a migration can be
applied again:

```sql
SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title) AS data
    FROM app.tb_post p $$, options => '{"logged": true, "fillfactor": 85}');
-- created | unchanged | altered (storage only) | replaced (same columns, rows
-- reconciled in place) | rebuilt
```

It runs the DDL as the caller and requires owning an existing TVIEW; a role that owns
the schema and has `TRIGGER` on the base tables needs no superuser. `CREATE TABLE
tv_post AS SELECT …` (optionally `UNLOGGED`, `WITH (fillfactor = n)`) runs the same
code with create-only semantics. See
[docs/reference/read-contract.md](docs/reference/read-contract.md).

#### Reading what is registered

Tools read `tviews.registry` (one row per TVIEW: schema, name, entity, normalized
query, base tables, options, `needs_reregister`, backing view) and check
`tviews.contract_version()`. Both follow the stability rules in
[docs/reference/read-contract.md](docs/reference/read-contract.md);
`tviews.pg_tview_meta` and the other `pg_tview_*` tables are internal.

#### Upgrading

Each release has its own extension version (`SELECT extversion FROM pg_extension
WHERE extname = 'pg_tviews'`) and ships upgrade scripts:

1. Install the new package and restart PostgreSQL (the library is preloaded).
2. In each database: `ALTER EXTENSION pg_tviews UPDATE;`
3. When the release notes say so, or `tviews.pg_tviews_health_check()` reports TVIEWs
   to re-register: `SELECT * FROM tviews.pg_tviews_reregister_all();` It re-derives
   each TVIEW's metadata and triggers from its definition, without touching its rows.

Between steps 1 and 2, writes to the TVIEWs' base tables fail with
`pg_tviews library catalog revision … does not match the installed extension`: they
are never served by a mismatched library.

Installs of `0.1.0` (every release up to 0.1.0-beta.19) cannot be updated in place.
After step 1, run [`scripts/migrate-from-0.1.0.sql`](scripts/migrate-from-0.1.0.sql) in
each database instead: it moves the extension to `tviews` and re-registers every
TVIEW, keeping their rows (not the audit log).

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
-- Monitor system health
SELECT * FROM pg_tviews_health_check();

-- Size, rows and indexes of each TVIEW
SELECT * FROM pg_tviews_performance_stats();
```

---

## ⏸️ Bulk Operations (Issue #44)

For bulk INSERT/UPDATE/DELETE operations (e.g., seed data loading, ETL imports) on tables with TVIEWs, use the suspend/resume API to prevent trigger-based refresh during the operation:

### Basic Pattern

Suspension lasts until the end of the transaction, so run the load in one:

```sql
BEGIN;
SELECT pg_tviews_suspend_triggers();   -- no refresh from here on in this transaction

INSERT INTO customers SELECT * FROM staging_customers;
INSERT INTO orders SELECT * FROM staging_orders;

SELECT pg_tviews_resume_triggers();    -- rebuilds the changed TVIEWs, dependencies first
COMMIT;
```

### Why This Matters

When bulk inserting into multiple related tables, triggers refresh TVIEW rows after
each statement, while the related tables may not be loaded yet. With suspension,
all the data is loaded first, and each TVIEW that changed (and every TVIEW that
embeds it) is rebuilt once, in dependency order.

### Ending the Transaction Without Resuming

An explicit `COMMIT` catches up the same way when the transaction is still
suspended. An implicit commit (an autocommit statement, a `DO` block) cannot: it
logs a WARNING naming the stale TVIEWs, to be fixed with `pg_tviews_refresh(entity)`
or `pg_tviews_refresh_all()`.

### API Reference

- `pg_tviews_suspend_triggers()` - Start suspension (supports nesting)
- `pg_tviews_resume_triggers()` - Resume; rebuilds the TVIEWs that changed and those that embed them
- `pg_tviews_refresh(entity)` - Rebuild one TVIEW and every TVIEW that embeds it, in dependency order
- `pg_tviews_refresh_all()` - Rebuild every TVIEW in dependency order
- `pg_tviews_is_suspended()` - Check current suspension state
- `pg_tviews_suspended_entities()` - List entities that changed during suspension

### Nested Suspension

Calls can be nested; each must be matched:

```sql
BEGIN;
SELECT pg_tviews_suspend_triggers();  -- depth 1
SELECT pg_tviews_suspend_triggers();  -- depth 2
-- operations...
SELECT pg_tviews_resume_triggers();   -- depth 1
SELECT pg_tviews_resume_triggers();   -- depth 0 (now resumed)
COMMIT;
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
- **[Error Reference](docs/error-reference.md)** - Error types and solutions
- **[Configuration](#configuration)** - GUC settings

### Operations
- **[Monitoring](docs/operations/monitoring.md)** - Metrics and health checks
- **[Troubleshooting](docs/operations/troubleshooting.md)** - Debugging procedures
- **[Performance](docs/operations/performance-tuning.md)** - 📊 Performance tuning
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
- **[Development](docs/development.md)** - Development setup
- **[Testing](docs/development/testing.md)** - Testing patterns and procedures
- **[Architecture](architecture.md)** - Technical architecture

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

**Development Setup**: See [docs/development.md](docs/development.md)

---

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

<div align="center">

**⭐ If you find this project interesting, please consider starring it! ⭐**

*Built with ❤️ and Rust 🦀*

</div>
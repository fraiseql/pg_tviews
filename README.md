# pg_tviews

<div align="center">

**Transactional Materialized Views with Incremental Refresh for PostgreSQL**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![PostgreSQL](https://img.shields.io/badge/PostgreSQL-16%E2%80%9318-blue.svg)](https://www.postgresql.org/)
[![Rust](https://img.shields.io/badge/Rust-1.98-orange.svg)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-0.1.0--beta.35-orange.svg)](https://github.com/fraiseql/pg_tviews/releases)
[![Status](https://img.shields.io/badge/status-beta-blue.svg)](https://github.com/fraiseql/pg_tviews/releases)

**CI/CD Status**:
[![CI](https://github.com/fraiseql/pg_tviews/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/ci.yml)
[![Clippy Strict](https://github.com/fraiseql/pg_tviews/actions/workflows/clippy.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/clippy.yml)
[![Coverage](https://github.com/fraiseql/pg_tviews/actions/workflows/coverage.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/coverage.yml)
[![Security Audit](https://github.com/fraiseql/pg_tviews/actions/workflows/security-audit.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/security-audit.yml)
[![Documentation](https://github.com/fraiseql/pg_tviews/actions/workflows/docs.yml/badge.svg?branch=main)](https://github.com/fraiseql/pg_tviews/actions/workflows/docs.yml)

*Core infrastructure for FraiseQL's GraphQL Cascade: incremental refresh of JSONB read models, inside the writing transaction.*

By Lionel Hamayon • Part of the FraiseQL framework

[Features](#-key-features) •
[Quick Start](#-quick-start) •
[Performance](#-performance) •
[Documentation](#-documentation) •
[Architecture](#-architecture)

</div>

> **Where it runs.** pg_tviews is a compiled extension loaded with
> `shared_preload_libraries`: it runs on PostgreSQL you install or operate yourself,
> including on Kubernetes (operator image). Managed services (RDS, Cloud SQL, Azure,
> Supabase, Neon, …) only install extensions from their own allow-lists; there, use
> FraiseQL's `v_*` mode. See [ADR 0232](docs/adr/0232-distribution.md).

---

## 🍓 Part of the FraiseQL Ecosystem

**pg_tviews** is the performance foundation for FraiseQL's CQRS architecture:

### **Server Stack (PostgreSQL + Python/Rust)**

| Tool | Purpose | Status |
|------|---------|--------|
| **[pg_tviews](https://github.com/fraiseql/pg_tviews)** | Incremental materialized views | Beta |
| **[jsonb_delta](https://github.com/evoludigit/jsonb_delta)** | JSONB patch functions (required by pg_tviews) | Stable |
| **[pgGit](https://github.com/evoludigit/pgGit)** | Database per branch or agent, attributed DDL history (not a schema source of truth; promotes into confiture) | In development (v2) |
| **[confiture](https://github.com/fraiseql/confiture)** | PostgreSQL migrations | Stable |
| **[fraiseql](https://fraiseql.dev)** | GraphQL framework | Stable |
| **[fraiseql-data](https://github.com/fraiseql/fraiseql-seed)** | Seed data generation | Planned |

### **Client Libraries (TypeScript/JavaScript)**

| Library | Purpose | Framework Support |
|---------|---------|-------------------|
| **[graphql-cascade](https://github.com/graphql-cascade/graphql-cascade)** | Automatic cache invalidation | Apollo, React Query, Relay, URQL |

**How pg_tviews fits:**
- **fraiseql** uses pg_tviews for GraphQL read models (tv_* tables)
- **jsonb_delta** patches the stored documents in place (required)
- **confiture** manages TVIEW schema evolution
- **graphql-cascade** (client-side) invalidates browser caches when mutations trigger refreshes

**Stack it up:**
```bash
# Install extensions
CREATE EXTENSION pg_tviews CASCADE;  -- installs jsonb_delta, which it requires

# Create incremental view
CREATE TABLE tv_post AS SELECT ...;

# Use with fraiseql GraphQL
@fraiseql.type(sql_source="tv_post")
class Post: ...
```

---

## 📋 Version Status

**Current Version**: `0.1.0-beta.35` (November 2026)
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

**Result**: each write refreshes only the TVIEW rows it reaches, in the same transaction, with nothing to schedule.

### jsonb_delta

pg_tviews requires the `jsonb_delta` extension (`requires` in its control file):

```sql
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;     -- or CREATE EXTENSION pg_tviews CASCADE
```

It does the nested merges of the patches (a child's document into the documents
embedding it), measured faster than core JSONB per call on a 23 KB document (5.02 µs
against 6.37 µs). Writing one value into many rows keeps core `jsonb_set`, measured
faster there (23 KB: 4.14 µs against 4.81 µs; 490 B: 0.48 µs against 1.15 µs). See
[installation](docs/getting-started/installation.md#jsonb_delta).

---

## 🔑 Keys

A TVIEW's keys are a contract ([DDL reference](docs/reference/ddl.md#keys)):

- `pk_<entity>` (`smallint`, `integer`, `bigint` or a domain over one) is the row key
  and the TVIEW table's primary key; the first `pk_*` column names the entity. Another
  type is refused (`42804`).
- `id` (`uuid`) is the public identifier, the one `pg_tviews_flush_and_report()` and
  the change feed carry.
- `fk_<entity>` holds the `pk_<entity>` of the row it references, and joins on it.

A `DISTINCT ON` TVIEW is keyed on its `DISTINCT ON` key instead. Base tables and join
columns may have any names: pg_tviews reads the definition's query tree, not its
spelling. FraiseQL adds `identifier` (a text slug) and `<parent>_id` (a parent's uuid,
for filtering), which are ordinary columns.

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
- **🤝 Correct Under Concurrency**: Concurrent writers to related rows wait for each other on the join values they share, so every TVIEW row equals its view once they commit; writers run at `READ COMMITTED` (other levels are refused with `0A000`). See [Concurrency](docs/concurrency.md)
- **📊 Cascade Propagation**: Automatically handles multi-level view dependencies

### Write path

- **📦 One refresh per statement**: a statement's changed rows are mapped to TVIEW keys
  together, and each reached row is refreshed once, when the statement ends
- **🎨 Patches**: a write that only changes values copied into `data` patches the
  documents in place instead of recomputing them
- **🚀 UNLOGGED Tables, opt-in**: no WAL for refreshes, refilled automatically after a crash

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
- **⏸️ Bulk Operations**: Suspend/resume triggers for safe bulk data loading

---

## 📊 Performance

Single-row `UPDATE` on a product catalogue TVIEW, median ms, against `REFRESH
MATERIALIZED VIEW` of the same view ([results](docs/benchmarks/results.md), measured
on 0.1.0 with PostgreSQL 18.4):

| Products / reviews | pg_tviews | full refresh |
|---|---|---|
| 1K / 5K | 1.77 | 15.0 |
| 10K / 50K | 1.87 | 167.9 |
| 100K / 500K | 1.97 | 1,523.5 |

A refresh costs what the write reaches, not the size of the TVIEW. The FraiseQL-shaped
write benchmarks are in [write path](docs/benchmarks/write-path.md), reads in
[read path](docs/benchmarks/read-path.md).

---

## Patches instead of recomputes

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
- the entity is not `DISTINCT ON` and not a `UNION`;
- every changed column maps identity-style to a top-level `data` key
  (`jsonb_build_object('bio', bio)` — no expression, cast, or other relation);
- no changed column is a foreign key, the primary key, or a column the tview also
  projects **outside** `data`;
- the changed values' types are TEXT/VARCHAR, INT2/4/8, BOOL, UUID, JSONB, or NULL
  (anything else — float, numeric, timestamps, arrays — falls back);
- for a parent, its dependency on the child is `nested_object` with a path
  (array/scalar/UUID-fk parents recompute).

### Copied parent columns

A tview that copies a column of a joined parent table into its `data`, at the top
level or in nested objects as FraiseQL writes them
(`'author', jsonb_build_object('name', u.name)` over `JOIN tb_user u ON u.pk_user =
p.fk_user`), is written by **one statement** when that column changes:
`UPDATE tv_post … WHERE fk_user = <pk>`, `jsonb_set` at each path, instead of
recomputing every post of that user. Same rules as above, plus: the tview reads the
parent through one join on its own base table, projects that `fk_*` column, reads
the parent table only once, and uses the column nowhere else in its definition.
Arrays of embedded rows and embeds two joins away recompute. On the FraiseQL-shaped
[write benchmark](docs/benchmarks/write-path.md) an author's bio change costs ~0.15 ms
over the plain UPDATE.

### Kill-switch and observability

```sql
SET pg_tviews.direct_patch_enabled = off;   -- superuser: force the recompute path everywhere
```

`tviews.stats` exposes the counters per TVIEW, from any session:

```sql
SELECT entity, patch_captured, patch_applied, patch_fallbacks, view_recomputes
FROM tviews.stats;
```

An eligible update leaves `view_recomputes` unchanged and bumps `patch_applied` —
proof it skipped the view. `pg_tviews_show_queue()` shows the same counters for the
calling session only.

---

## 💾 LOGGED and UNLOGGED TVIEWs

TVIEWs are **LOGGED** tables by default: crash-safe, replicated, readable on a hot
standby. A TVIEW whose rows can be recomputed and that needs the write speed declares
`logged: false` and becomes an **UNLOGGED** table (no WAL for its refreshes).

```sql
SELECT pg_tviews_create_or_replace('tv_post', $$ SELECT … $$, '{"logged": false}');
CREATE UNLOGGED TABLE tv_tag AS SELECT …;                 -- the same, as DDL
SELECT * FROM pg_tviews_show_replication();             -- what a standby can serve
```

`ALTER TABLE tv_post SET LOGGED` switches one back, like the `logged` option.

### ⚠️ Hot standbys, promotion and crash restarts

An UNLOGGED table is not replicated. **A hot standby cannot read an UNLOGGED
TVIEW at all** (`ERROR: cannot access temporary or unlogged relations during
recovery`), and promotion or a crash restart leaves it empty. **pg_tviews** records
which UNLOGGED TVIEWs it can trust in an UNLOGGED table the reset empties too. After a
crash restart or a promotion, a background worker refills the reset TVIEWs of every
database (`pg_tviews.auto_rebuild_databases`, default `*`); a write that reaches a
reset TVIEW first refills it itself. A TVIEW that is merely empty is never refilled.
After a restore, call `SELECT * FROM pg_tviews_rebuild_all();`. See
[docs/operations/replication.md](docs/operations/replication.md).

### Configuration

A TVIEW is its definition and its options ([DDL reference](docs/reference/ddl.md)):
no setting changes what it is. There are five settings:

| GUC | Type | Default | Who sets it | Purpose |
|-----|------|---------|-------------|---------|
| `pg_tviews.publish_changes` | enum | off | superuser | Publish changed TVIEW rows at commit on `tviews_changes`: `off`, `types`, `rows` ([change feed](docs/user-guides/change-feed.md)) |
| `pg_tviews.max_refresh_rows` | int | 0 | superuser | TVIEW rows one statement's refresh may write; 0 = no limit ([ADR 0233](docs/adr/0233-fanout-cost-and-guard.md)) |
| `pg_tviews.lock_escalation_threshold` | int | 64 | superuser | Value locks a transaction takes on one relation before it locks the relation instead (minimum 0 = always the relation; see [Concurrency](docs/concurrency.md)) |
| `pg_tviews.audit_enabled` | bool | off | superuser | Audit logging (opt-in) |
| `pg_tviews.auto_rebuild_databases` | string | `*` | `postgresql.conf` (restart) | Databases whose reset UNLOGGED TVIEWs are refilled after a crash restart or a promotion: `*` every one, a list only those, empty none |

The superuser-only diagnostics `pg_tviews.graph_cache_enabled`,
`pg_tviews.table_cache_enabled` and `pg_tviews.direct_patch_enabled` (all on) are
hidden from `SHOW ALL`; results are the same either way. So is
`pg_tviews.max_queue_size`, the refresh keys a transaction may queue (1,000,000): a
memory bound, raised only for a bulk change that queues more, which is better made
after `pg_tviews_suspend_triggers()`. Diagnostics are `DEBUG1` messages:
`SET client_min_messages = debug1` shows them.

Other limits are fixed, and named by their errors: a flush applies at most 1,000
entity batches (only a loop reaches it: a trigger on a TVIEW table writing back what
the refresh reads), a definition nests at most 32 query levels and 64 views, bulk refreshes run in batches
of 1,000 keys, each in-memory memo keeps 10,000 entries per backend, and
`pg_tviews_flush_and_report()` keeps 10,000 changed rows per transaction. The
`pg_tviews.` prefix is reserved: setting any other name under it fails.

> The settings require `shared_preload_libraries = 'pg_tviews'`, which the extension
> needs anyway.

### What a definition needs

A TVIEW's definition is one `SELECT` that outputs `pk_<entity>` and reads at least
one table. Every table it reads either maps its writes to the TVIEW's keys or
follows the TVIEW's `uncascaded_policy`: under the default, `error`, a definition
reading a table whose writes cannot be traced is refused at creation, with the
reason and how to declare a full refresh instead. [Supported
shapes](docs/reference/shapes.md) lists each shape and what pg_tviews does with it.

### Safety Guarantees

- **✅ Data Recovery**: All TVIEW data reconstructible from base tables
- **✅ Transparent**: Applications work unchanged
- **✅ Opt-in**: UNLOGGED only for the TVIEWs that declare it
- **✅ Only after a reset**: An empty TVIEW is refilled only when PostgreSQL reset it, once, whatever the number of concurrent writers
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

# Preload the library, then restart PostgreSQL
#   postgresql.conf: shared_preload_libraries = 'pg_tviews'

# Enable in your database (as a superuser)
psql -d your_database -c "CREATE EXTENSION pg_tviews CASCADE;"
```

Preloading is the only supported mode. Without it `CREATE EXTENSION pg_tviews` fails
with `55000` (`pg_tviews is not in shared_preload_libraries`), and so does every
function and catalog write of an installed extension whose preload was removed.
`CREATE EXTENSION` needs a superuser: the script creates event triggers and C
functions.

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
    FROM app.tb_post p $$, options => '{"fillfactor": 90}');
-- created | unchanged | altered (options only) | replaced (same columns, rows
-- reconciled in place) | rebuilt
```

The options passed are the whole declaration: an option left out is at its default
(`logged: true`, `fillfactor: 85`, …), also on an existing TVIEW.

It runs the DDL as the caller and requires owning an existing TVIEW; a role that owns
the schema and has `TRIGGER` on the base tables needs no superuser. `CREATE TABLE
tv_post AS SELECT …` (optionally `UNLOGGED`, `WITH (fillfactor = n, toast_tuple_target = n)`) runs the same
code with create-only semantics. See
[docs/reference/read-contract.md](docs/reference/read-contract.md).

#### Reading what is registered

Tools read `tviews.registry` (one row per TVIEW: schema, name, entity, normalized
query, every option, base tables, `needs_reregister`, backing view), `tviews.stats`
(refresh counters per TVIEW) and check `tviews.contract_version()` (3). All three follow the
stability rules in
[docs/reference/read-contract.md](docs/reference/read-contract.md);
`tviews.pg_tview_meta` and the other `pg_tview_*` tables are internal.

#### Upgrading

A release updates from the release before it, with one script:

1. Pause the writers, install the new package and restart PostgreSQL (the library is
   preloaded).
2. In each database: `ALTER EXTENSION pg_tviews UPDATE;` It re-derives every TVIEW's
   plan and triggers from its definition, without touching its rows; a TVIEW that no
   longer analyses fails the whole update and nothing is changed. jsonb_delta must be
   installed.

Between steps 1 and 2, writes to the TVIEWs' base tables fail with
`pg_tviews library catalog revision … does not match the installed extension`: they
are never served by a mismatched library. Reads keep working.

An install of an older release, `0.1.0` installs included, is not updated: drop the
extension and create the TVIEWs again; the base tables keep their rows. Both
procedures: [docs/operations/upgrade.md](docs/operations/upgrade.md).

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
SELECT * FROM pg_tviews_check_health();

-- Size, HOT ratio and missing indexes of each TVIEW
SELECT * FROM pg_tviews_profile();

-- What each TVIEW's refreshes did, from any session
SELECT * FROM tviews.stats;
```

---

## ⏸️ Bulk Operations

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
suspended. An implicit commit (several statements sent as one query string, a
`COMMIT` inside a procedure) cannot: while writes are suspended it fails with `55000`
and the transaction rolls back, so resume before it commits.

### API Reference

- `pg_tviews_suspend_triggers()` - Start suspension (supports nesting)
- `pg_tviews_resume_triggers()` - Resume; rebuilds the TVIEWs that changed and those that embed them
- `pg_tviews_refresh(tview)` - Rebuild one TVIEW and every TVIEW that embeds it, in dependency order
- `pg_tviews_refresh_all()` - Rebuild every TVIEW in dependency order
- `pg_tviews_show_suspended()` - The TVIEWs changed while suspended; NULL when not suspended

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

```
INSERT / UPDATE / DELETE on a base table
        │
        ▼
row trigger, or statement triggers with transition tables
        │  the TVIEW keys the changed rows reach, queued
        ▼
flush trigger at the end of the statement
(also the ProcessUtility hook before COMMIT and PREPARE TRANSACTION)
        │
        ▼
flush, TVIEW by TVIEW in dependency order:
  patch the documents in place, or recompute the rows through the backing view;
  queue the rows of TVIEWs that embed what changed
        │
        ▼
tv_* rows written in the writing transaction
(the change feed is published at commit when pg_tviews.publish_changes is on)
```

### Key Components

1. **Lineage**: reads each definition's query tree at create time and stores, per base
   table, how a changed row maps to TVIEW keys (`tviews.registry.cascade_kinds`)
2. **Triggers**: queue those keys, and flush at the end of each statement
   ([firing order](docs/reference/ddl.md#firing-order))
3. **Flush**: refreshes the queued keys in dependency order, with value locks so
   concurrent writers agree ([Concurrency](docs/concurrency.md))
4. **Backing views**: each definition stored as a view in `tviews`, which recomputes
   a row
5. **Monitoring**: `pg_tviews_check_health()`, `tviews.stats`, `pg_tviews_profile()`

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
- **[Embed or Compose](docs/user-guides/embed-or-compose.md)** - Embed a list in the parent's document, or compose it at read time
- **[Change Feed](docs/user-guides/change-feed.md)** - Cache invalidation across processes (`tviews_changes`)

### Reference
- **[API Reference](docs/reference/api.md)** - Complete function reference
- **[DDL Reference](docs/reference/ddl.md)** - CREATE/DROP TABLE syntax
- **[Supported Shapes](docs/reference/shapes.md)** - Each definition shape, maintained or refused, with its tests
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

❌ **Write-Heavy Tables** - Every write pays for the TVIEW rows it reaches, in its own transaction
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
# Installation Guide

Building pg_tviews from source, enabling it in a database, upgrading and removing it.

## Where it runs

pg_tviews is a compiled extension that PostgreSQL must preload
(`shared_preload_libraries`). It runs where you install the server software yourself:

- **self-hosted PostgreSQL** 16, 17 or 18 (build from source, or the release tarball);
- **Kubernetes**: the operator mounts the extension's files into the PostgreSQL image
  (CloudNativePG 1.27+ image-volume extensions on PostgreSQL 18, with
  `extension_control_path`; a custom operand image otherwise) and preloads the library.

Managed services (Amazon RDS and Aurora, Cloud SQL, AlloyDB, Azure Database for
PostgreSQL, Supabase, Neon, Crunchy Bridge) install extensions from their own lists, and
pg_tviews is on none of them. On those, use FraiseQL's `v_*` mode: the same
queries over plain views. Why, and what a SQL-only edition would cost:
[ADR 0232](../adr/0232-distribution.md).

## Requirements

- **PostgreSQL** 16, 17 or 18, with its server development files (`pg_config`);
  `CREATE EXTENSION` refuses older versions
- **Rust**: the toolchain pinned in `rust-toolchain.toml` (rustup installs it on the
  first build)
- **cargo-pgrx** 0.17.0, the version the extension is built with
- **jsonb_delta** 0.3.0 or later: required (below); `CREATE EXTENSION pg_tviews CASCADE`
  installs it when its files are on the server

## 1. Build and install

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# pgrx, at the version pg_tviews uses
cargo install --locked cargo-pgrx --version 0.17.0

# Point pgrx at the PostgreSQL you install into (here 17)
cargo pgrx init --pg17 "$(command -v pg_config)"

# Build and install into that PostgreSQL
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews
cargo pgrx install --release --pg-config "$(command -v pg_config)" \
    --no-default-features --features pg17
```

The default feature is `pg18`: for PostgreSQL 18, leave out
`--no-default-features --features …`; for 16, use `pg16`. Pass `--pg-config`
explicitly so the files go to the server you run, not to a PostgreSQL that pgrx
manages.

The server development files come from your distribution:

| Platform | Package |
|---|---|
| Debian, Ubuntu (PGDG) | `postgresql-server-dev-17` |
| RHEL, Rocky (PGDG) | `postgresql17-devel` |
| macOS (Homebrew) | `postgresql@17` |

## 2. Load the library

pg_tviews installs its hooks (the interception of `CREATE TABLE tv_* AS`, `DROP TABLE
tv_*` and `COMMIT`), its shared memory and its settings when the server starts.
Preload it in `postgresql.conf`, then restart PostgreSQL:

```ini
shared_preload_libraries = 'pg_tviews'
```

Preloading is the only supported mode. Without it `CREATE EXTENSION pg_tviews` fails
with `55000` (`pg_tviews is not in shared_preload_libraries`, hint: add it and
restart). If the preload is removed after the extension is installed, every pg_tviews
function and every write of its catalog fails the same way.

## 3. Create the extension

```bash
psql -d your_database
```

```sql
-- Required by pg_tviews (CASCADE below installs it too)
CREATE EXTENSION IF NOT EXISTS jsonb_delta;

-- Its objects go to the schema tviews
CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

SELECT tviews.pg_tviews_version();
```

`CREATE EXTENSION pg_tviews` needs a superuser: its script creates event triggers and C
functions, and the control file says `superuser = true`. Every object goes to the
schema `tviews`; to call the functions unqualified, add it to the database's
`search_path` (it applies to new sessions):

```sql
DO $$ BEGIN
    EXECUTE format('ALTER DATABASE %I SET search_path = "$user", public, tviews',
                   current_database());
END $$;
```

## jsonb_delta

pg_tviews requires `jsonb_delta` (`requires` in its control file): `CREATE EXTENSION
pg_tviews` without it fails with PostgreSQL's "required extension is not installed",
and `jsonb_delta` cannot be dropped while pg_tviews is installed. An eligible
`UPDATE` (a changed column copied as-is into `data`, a child's document embedded in its
parents) patches the stored documents in place instead of recomputing them; the result
is the same document.

The patches use two functions, measured per call:

| Patch | Function | 23 KB document | 490 B document |
|---|---|---|---|
| A row's own document, nested merge (a child's document into its parent) | `jsonb_delta` | 5.02 µs (core JSONB: 6.37 µs) | |
| One value written into many rows (a copied parent column, fan-out) | core `jsonb_set` | 4.14 µs (`jsonb_delta`: 4.81 µs) | 0.48 µs (`jsonb_delta`: 1.15 µs) |

`jsonb_delta` wins the nested merge, where core JSONB names the document twice; core
`jsonb_set` is faster for a one-value write, so the fan-out keeps it. A row's own
top-level merge also goes through `jsonb_delta`, which is not faster there (23 KB:
4.48 µs against 4.20 µs for core `||`). It is required rather than optional because one
mode is simpler to keep correct than two. Its contract with pg_tviews is tested by
`test/sql/regress/direct_patch/regress_jsonb_delta_semantics.sql`.

## Verification

```sql
\dx pg_tviews

SELECT * FROM tviews.pg_tviews_check_health();

CREATE TABLE tb_install_check (pk_install_check bigint PRIMARY KEY, label text);
CREATE TABLE tv_install_check AS
SELECT pk_install_check, jsonb_build_object('label', label) AS data
FROM tb_install_check;
INSERT INTO tb_install_check VALUES (1, 'it works');
SELECT data FROM tv_install_check;        -- {"label": "it works"}
DROP TABLE tv_install_check;
DROP TABLE tb_install_check;
```

## Production notes

**Connection poolers.** Refresh work is queued and flushed inside the writing
transaction, so PgBouncer in `transaction` pool mode works. What a TVIEW is comes from
its definition and its options, never from a session setting, and
`pg_tviews_suspend_triggers()` lasts one transaction at most.

**Replication.** TVIEW tables are LOGGED by default: crash-safe and readable on a hot
standby. A TVIEW declared `options => '{"logged": false}'` (or created with
`CREATE UNLOGGED TABLE tv_x AS`) writes faster, cannot be read on a standby, and is
empty after a crash or a promotion until a background worker refills it. See
[Replication](../operations/replication.md).

**Several servers.** Build once per PostgreSQL major version and copy the files
`cargo pgrx package` produces (`pg_tviews.so` to `pg_config --pkglibdir`,
`pg_tviews.control` and `pg_tviews--*.sql` to `pg_config --sharedir`/extension).

## Troubleshooting installation

| Symptom | Check |
|---|---|
| `extension "pg_tviews" is not available` | the files are not in this server's directories: `pg_config --pkglibdir --sharedir` of the server you run, and reinstall with `--pg-config` |
| `could not access file "pg_tviews"` at startup | `shared_preload_libraries` names a library that is not installed for this server |
| `55000` `pg_tviews is not in shared_preload_libraries` | add `pg_tviews` there and restart |
| `permission denied to create extension "pg_tviews"` | create it as a superuser |
| `function pg_tviews_… does not exist` | qualify it (`tviews.pg_tviews_…`) or add `tviews` to the `search_path` |
| permission denied (`42501`) on a maintenance function | it is not executable by `PUBLIC`: [Operator role](../user-guides/operators.md#operator-role) |
| `cargo pgrx` version mismatch | `cargo install --locked cargo-pgrx --version 0.17.0 --force` |

## Upgrading

```bash
cd pg_tviews && git pull
cargo pgrx install --release --pg-config "$(command -v pg_config)"   # plus the feature flags above
sudo systemctl restart postgresql                                      # the library is preloaded
```

Then, in each database:

```sql
ALTER EXTENSION pg_tviews UPDATE;
```

The update re-derives every TVIEW from its stored definition and fails, naming the
TVIEW, when one no longer analyses. It starts from the previous release; an older
install is dropped and recreated ([Upgrading pg_tviews](../operations/upgrade.md)).
Until the database is updated, writes to base tables and pg_tviews functions fail with
`55000`, naming the fix.

## Uninstalling

```sql
DROP EXTENSION pg_tviews CASCADE;
```

This drops the triggers and the backing views; the `tv_*` tables stay as plain tables
with their rows (drop them if you no longer need them). Then remove `pg_tviews` from
`shared_preload_libraries` and restart.

## Next Steps

- **[Quick Start](quickstart.md)** - Create your first TVIEW
- **[DDL Reference](../reference/ddl.md)** - What a definition may contain
- **[Monitoring](../operations/monitoring.md)** - Production monitoring setup
- **[Development](../development.md)** - Building and running the test suites

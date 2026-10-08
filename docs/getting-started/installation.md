# Installation Guide

Building pg_tviews from source, enabling it in a database, upgrading and removing it.

## Requirements

- **PostgreSQL** 16, 17 or 18, with its server development files (`pg_config`);
  `CREATE EXTENSION` refuses older versions
- **Rust**: the toolchain pinned in `rust-toolchain.toml` (rustup installs it on the
  first build)
- **cargo-pgrx** 0.17.0, the version the extension is built with
- **jsonb_delta** (optional): enables the direct-patch fast path (below)

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
tv_*` and `COMMIT`) when its library is loaded. Load it with the server, in
`postgresql.conf`, then restart PostgreSQL:

```ini
shared_preload_libraries = 'pg_tviews'
```

Without it, a `CREATE TABLE tv_* AS` that reaches the server un-intercepted fails,
naming the fix; the SQL functions still work.

## 3. Create the extension

```bash
psql -d your_database
```

```sql
-- Optional, before or after pg_tviews: the direct-patch fast path
CREATE EXTENSION IF NOT EXISTS jsonb_delta;

-- Its objects go to the schema tviews
CREATE EXTENSION IF NOT EXISTS pg_tviews;

SELECT tviews.pg_tviews_version();
SELECT tviews.pg_tviews_check_jsonb_delta();   -- true when jsonb_delta is installed
```

`CREATE EXTENSION pg_tviews` needs `CREATE` on the database. Every object goes to the
schema `tviews`; to call the functions unqualified, add it to the database's
`search_path` (it applies to new sessions):

```sql
DO $$ BEGIN
    EXECUTE format('ALTER DATABASE %I SET search_path = "$user", public, tviews',
                   current_database());
END $$;
```

## jsonb_delta

pg_tviews works without other extensions. Without `jsonb_delta`, every TVIEW row a
write affects is recomputed from its backing view, and `CREATE EXTENSION pg_tviews`
says so with a WARNING. With it, an eligible single-row `UPDATE` (a changed column
copied as-is into `data`, a child's document embedded in its parents) patches the
stored documents in place instead of recomputing them; the result is the same
document. The conditions and the measured figures are in the
[README](../../README.md) and [Benchmark results](../benchmarks/results.md).
`SET pg_tviews.direct_patch_enabled = off` turns the fast path off.

## Verification

```sql
\dx pg_tviews

SELECT * FROM tviews.pg_tviews_health_check();

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
transaction, so PgBouncer in `transaction` pool mode works. Session settings
(`SET pg_tviews.uncascaded_policy`, `SET pg_tviews.suspend_triggers`) follow the usual
pooling rules: set them in the transaction that needs them (`SET LOCAL`).

**Replication.** TVIEW tables are `UNLOGGED` by default
(`pg_tviews.unlogged_by_default = on`): a hot standby cannot read them, and they are
empty after a crash or a promotion until rebuilt. Create the TVIEWs a standby serves
with `options => '{"logged": true}'`, or switch one with
`tviews.pg_tviews_set_logged(entity, true)`. See
[Replication](../operations/replication.md).

**Several servers.** Build once per PostgreSQL major version and copy the files
`cargo pgrx package` produces (`pg_tviews.so` to `pg_config --pkglibdir`,
`pg_tviews.control` and `pg_tviews--*.sql` to `pg_config --sharedir`/extension).

## Troubleshooting installation

| Symptom | Check |
|---|---|
| `extension "pg_tviews" is not available` | the files are not in this server's directories: `pg_config --pkglibdir --sharedir` of the server you run, and reinstall with `--pg-config` |
| `could not access file "pg_tviews"` at startup | `shared_preload_libraries` names a library that is not installed for this server |
| `CREATE TABLE tv_* AS` fails asking for `shared_preload_libraries` | add `pg_tviews` there and restart |
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
TVIEW, when one no longer analyses. It starts from 0.1.0-beta.20 or later
([Deprecation warnings](../DEPRECATION_WARNINGS.md)). Until the database is updated,
pg_tviews functions fail with `55000`, naming the fix.

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

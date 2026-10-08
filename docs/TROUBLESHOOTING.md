# pg_tviews Troubleshooting Guide

Common errors and what to do about them. Every error pg_tviews raises carries a
SQLSTATE; the [error reference](error-reference.md) lists them all. Operational
problems (stale rows, slow refreshes, recovery) are covered in
[operations/troubleshooting.md](operations/troubleshooting.md).

## Setup

### `CREATE TABLE tv_x AS SELECT …` fails asking for `shared_preload_libraries`

**Cause**: `CREATE TABLE tv_<entity> AS SELECT …` is turned into a TVIEW by a hook
installed when the library is loaded. Without `pg_tviews` in
`shared_preload_libraries`, a session that has not loaded the library does not see
the statement, and the statement fails instead of leaving a plain table behind.

**Solution**: add `pg_tviews` to `shared_preload_libraries` and restart, or create
the TVIEW with the function, which works in any session:

```sql
SELECT tviews.pg_tviews_create_or_replace('tv_test', 'SELECT pk_test, id, data FROM tb_test');
```

### A pg_tviews function does not exist

```
ERROR: function pg_tviews_create_or_replace(unknown, unknown) does not exist
```

**Cause**: the extension is not created in this database, or its schema `tviews` is
not on the `search_path`.

**Solution**: qualify the call (`tviews.pg_tviews_create_or_replace(…)`), add
`tviews` to the database's `search_path`, or create the extension:

```sql
CREATE EXTENSION IF NOT EXISTS pg_tviews;
```

### `relation "tv_product" does not exist`

The TVIEW is in a schema that is not on the `search_path`. Find it, then qualify the
name or set the `search_path`:

```sql
SELECT schema, name FROM tviews.registry WHERE name = 'tv_product';
```

### Every pg_tviews call fails with `55000` after an upgrade

The library and the installed extension are at different versions. Run
`ALTER EXTENSION pg_tviews UPDATE;` in the database; the hint names the fix.

## Creating a TVIEW

| Error | SQLSTATE | Cause and fix |
|---|---|---|
| `TVIEW tv_foo does not match its definition, which is keyed on pk_post` | `22023` | The first `pk_*` column names the entity: name the TVIEW after it, or alias the key `pk_foo` |
| `Invalid SELECT statement: no column to key the rows on …` | `42601` | Output a `pk_<entity>` column |
| `Invalid SELECT statement: a TVIEW is defined by exactly one SELECT` | `42601` | Remove the other statements |
| `column "pk_x" is of type bigint but expression is of type uuid` | `42804` | `pk_<entity>` is stored as `bigint`: key on an integer column |
| `TVIEW tv_post already exists` | `42P07` | Change it with `pg_tviews_create_or_replace()` |
| `writes to public.tb_flag would not refresh public.tv_report (…)` | `22023` | A table no condition links to the TVIEW's key: declare `uncascaded_policy` or `uncascaded_tables`, or join on a column pg_tviews can trace ([Tables no cascade reaches](reference/ddl.md#tables-no-cascade-reaches)) |
| `… calls public.f(), not immutable …` | `22023` | Declare the tables the function reads in `function_reads` ([Functions that read tables](reference/ddl.md#functions-that-read-tables)) |
| A definition reading `CURRENT_DATE`, `now()` … refused | `22023` | Declare `"time_refresh": "external"` and call `pg_tviews_refresh_time_dependent()` at the boundary ([Time-dependent TVIEWs](reference/ddl.md#time-dependent-tviews)) |
| `relations would read each other in a cycle: …` | `42P17` | Restructure the definitions |
| `the DISTINCT ON key of tv_x (…) names its rows, but …` | `0A000` | Project the `DISTINCT ON` key, a single column |

To see what an accepted TVIEW's writes refresh:

```sql
SELECT name, cascade_kinds, uncascaded_tables, uncascaded_policy
FROM tviews.registry ORDER BY name;
```

## Writing

### A commit fails: `cannot commit with N queued refreshes`

```
ERROR:  pg_tviews: cannot commit with 1 queued refreshes for {"post"}: they were never applied (missing flush trigger?)
```

**Cause** (`55000`): a row trigger queued refresh work that no statement-level flush
trigger applied, usually because a flush trigger was dropped or disabled. The commit
fails rather than leave the TVIEW stale.

**Solution**:

```sql
SELECT * FROM tviews.pg_tviews_health_check() WHERE status <> 'OK';
SELECT * FROM tviews.pg_tviews_reregister_all();   -- re-installs missing triggers
```

`pg_tviews_reregister_all()` is not executable by `PUBLIC`
([Operator role](user-guides/operators.md#operator-role)); the owner of one TVIEW can
run `tviews.pg_tviews_reregister('tv_post')`.

### A write fails naming a TVIEW, with a `pg_tviews_reregister` hint

The row trigger could not tell what to refresh: the TVIEW's stored plan does not
decode (a catalog edited by hand, a restore out of step). Run the hint, then check:

```sql
SELECT tviews.pg_tviews_reregister('tv_post');
SELECT * FROM tviews.pg_tviews_health_check() WHERE component = 'plans';
```

### `could not serialize access due to concurrent update` (`40001`)

A `REPEATABLE READ` or `SERIALIZABLE` transaction's refresh would have recomputed a
TVIEW row from a snapshot older than another writer's committed change. Retry the
transaction. See [Concurrency](concurrency.md).

### `permission denied` (`42501`)

Refreshing, replacing, dropping or re-registering a TVIEW requires owning it (or being a
member of its owner, or of the extension's owner). The functions acting on every TVIEW
need `GRANT EXECUTE`: see the [Operator role](user-guides/operators.md#operator-role).

## A TVIEW differs from its definition

Compare the table with its backing view, under the settings refreshes render values
with ([Rendering](reference/ddl.md#rendering)):

```sql
BEGIN;
SET LOCAL TimeZone = 'UTC'; SET LOCAL DateStyle = 'ISO, YMD'; SET LOCAL IntervalStyle = 'postgres';
SET LOCAL extra_float_digits = 1; SET LOCAL bytea_output = 'hex';
SELECT count(*) FROM (
    SELECT pk_post, data FROM tv_post
    EXCEPT SELECT pk_post, data FROM tviews.public__tv_post) d;
COMMIT;
```

Rows differ when a table was written while refreshes were suspended
(`pg_tviews.suspend_triggers`), under the `warn` policy, or by a path no trigger sees.
Recompute the TVIEW in full (it takes `ACCESS EXCLUSIVE` on the table):

```sql
SELECT tviews.pg_tviews_refresh('post');
```

## Benchmarks

The benchmark harness is `test/sql/real_benchmark/`; see its README.

```bash
cd test/sql/real_benchmark
PGHOST=localhost PGPORT=28818 PGUSER=postgres ./run.sh --scales small 2>&1 | tee benchmark.log
```

psql variables (`:'name'`) are not interpolated inside `DO $$ … $$` bodies, which are
string literals: pass them in through a temporary table or `set_config()`, and let psql
do the quoting (`psql -v scale="$scale"`, not `-v scale="'$scale'"`).

## Getting help

Open an issue with what you ran, the full error (with its SQLSTATE, DETAIL and HINT:
`\set VERBOSITY verbose` in psql), `SELECT tviews.pg_tviews_version();`, and the output
of `SELECT * FROM tviews.pg_tviews_health_check();`.

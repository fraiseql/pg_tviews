# Testing

## Suites

| Suite | Where | What it checks |
|---|---|---|
| Unit | `#[test]` in `src/` | Pure Rust: plan decoding, SQL builders, graphs. No server. |
| Regression | `test/sql/regress/<feature>/regress_*.sql` | One behaviour per file, asserted with `RAISE EXCEPTION` and an `-- expect-output:` sentinel. |
| Integration | `test/sql/[0-9]*.sql` | Longer scenarios, each TVIEW checked against its backing view (`test/sql/lib/assert_fresh.sql`). |
| Differential | `test/sql/differential/` | Seeded random writes (MERGE, `ON CONFLICT`, COPY, transactions with savepoints) over every TVIEW shape; each TVIEW equals its view after every statement. |
| Isolation | `test/isolation/` | Concurrent writers, DDL and snapshot isolation levels, run by `pg_isolation_regress`. |
| Concurrency | `test/concurrency/run.sh` | pgbench workloads (uniform, hot keys, a bulk transaction, a single client): throughput, failed transactions, and rows that differ from the backing view afterwards. Run by hand, not in CI. |
| Upgrade | `test/upgrade/` | An older release upgraded to this tree: the catalog equals a fresh install's. |
| Documentation | `test/docs/run_doc_sql.sh` | The SQL of the user docs, run as written. |
| Benchmark | `test/sql/real_benchmark/` | Write latency and fan-out throughput against the real extension (not run in CI: see its README). |

Behaviour that needs a server is tested in SQL, not with `#[pg_test]`: a SQL test
runs against the installed extension exactly as users call it. CI
(`.github/workflows/ci.yml`) runs every SQL suite on PostgreSQL 16, 17 and 18, and
once more on 18 with another library's hooks loaded first (`pg_stat_statements`).

## Running the suites

```bash
# Unit tests. PostgreSQL symbols only resolve inside a server, so let the linker
# ignore them; a separate target dir keeps the main build cache valid.
CARGO_TARGET_DIR=target-unit RUSTFLAGS="-C link-arg=-Wl,--unresolved-symbols=ignore-all" \
  cargo test --lib --no-default-features --features pg18

# Lint and format (both enforced in CI)
cargo clippy --no-default-features --features pg18 --all-targets -- -D warnings
cargo fmt --check

# SQL suites, against a cluster with pg_tviews and jsonb_delta installed and
# pg_tviews in shared_preload_libraries
export PGHOST=localhost PGPORT=28818 PGUSER=postgres
./test/run_regression_tests.sh                        # every regress file
./test/run_regression_tests.sh regress_copy_from.sql  # one, by file name
./test/run_integration_tests.sh
./test/sql/differential/run.sh 60 "1 2 3"
PG_CONFIG=$(which pg_config) ./test/isolation/run.sh
RUNS=3 ./test/concurrency/run.sh uniform hot bulk     # stale rows must be 0
```

Each SQL test runs in a throwaway database whose `search_path` includes `tviews`.

## Writing a test

- A new behaviour gets a regress file in the feature directory it belongs to,
  named after the behaviour (`regress_<behaviour>.sql`, unique across
  `test/sql/regress/`). Its header says what it checks and why, and names the
  `-- expect-output: <name>: PASS` line it `\echo`es at the end. It fails by
  raising, never by printing something someone has to read.
- Check a TVIEW against its backing view with `assert_fresh(tv, key, label)` from
  `test/sql/lib/assert_fresh.sql` (`\ir ../../lib/assert_fresh.sql` from a regress
  file): it compares under the settings refreshes render values under.
- A test reproducing an open defect carries `-- known-failing: <issue>`: the runner
  reports it as XFAIL until it passes, then fails until the marker is removed.
- A concurrency property is an isolation spec with its expected output; one
  reproducing an open defect stays out of `isolation_schedule`
  (`test/isolation/README.md`).
- Prove a new test fails for the right reason before fixing the code: run it on the
  build without the fix.

## Coverage

`.github/workflows/coverage.yml` measures line coverage of the unit tests with
`cargo llvm-cov` (PostgreSQL 18); the SQL suites are not counted.

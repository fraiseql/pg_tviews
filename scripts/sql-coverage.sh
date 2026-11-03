#!/usr/bin/env bash
# Line coverage of the SQL suites: the regression and isolation suites run against
# an instrumented build of pg_tviews, merged with the unit tests.
#
# Builds the extension with `cargo llvm-cov show-env` (`-C instrument-coverage`),
# lays the package out with jsonb_delta's, starts a throwaway cluster that preloads
# it, with LLVM_PROFILE_FILE in the postmaster's environment so every backend writes
# its profile when it exits, runs the suites, stops the cluster (a fast shutdown
# exits the backends normally) and reports. A backend that crashes writes no
# profile: its lines are not counted.
#
# Usage: PG_CONFIG=/usr/lib/postgresql/18/bin/pg_config JSONB_DELTA_DIR=<jsonb_delta checkout> \
#          [UNIT_LINK_FLAGS='-C link-arg=-fuse-ld=mold'] [FAIL_UNDER_LINES=N] \
#          scripts/sql-coverage.sh [port] [outdir]
# Writes <outdir>/lcov.info and <outdir>/summary.txt; prints the TOTAL line. With
# FAIL_UNDER_LINES, fails when the line coverage is below it.
set -euo pipefail
port="${1:-28950}"
out="$(realpath -m "${2:-target/sql-coverage}")"
: "${PG_CONFIG:?PG_CONFIG names the pg_config of the server}"
: "${JSONB_DELTA_DIR:?JSONB_DELTA_DIR names a jsonb_delta checkout to build}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bindir="$("$PG_CONFIG" --bindir)"
work="$out/work"
# Only a directory this script made is emptied.
if [[ -e "$out" && ! -f "$out/.sql-coverage" ]]; then
  echo "refusing to empty $out: not made by sql-coverage.sh" >&2; exit 2
fi
rm -rf "$out"; mkdir -p "$work/pkg/lib" "$work/pkg/share/extension"; touch "$out/.sql-coverage"

# jsonb_delta, built plain: its coverage is not ours.
(cd "$JSONB_DELTA_DIR" && CARGO_TARGET_DIR="$work/jd-target" cargo pgrx package \
   --no-default-features --features pg18 --pg-config "$PG_CONFIG" --out-dir "$work/jd" >/dev/null)

cd "$root"
eval "$(cargo llvm-cov show-env --sh)"
cargo llvm-cov clean --workspace
cargo pgrx package --no-default-features --features "pg18 test-hooks" \
  --pg-config "$PG_CONFIG" --out-dir "$work/raw" >/dev/null
for d in "$work/raw" "$work/jd"; do
  find "$d" -name '*.so' -exec cp {} "$work/pkg/lib/" \;
  find "$d" -path '*extension/*' -type f -exec cp {} "$work/pkg/share/extension/" \;
done
sed -i 's|\$libdir/||' "$work/pkg/share/extension/"*.control

"$bindir/initdb" -D "$work/data" -U postgres -A trust >/dev/null
cat >> "$work/data/postgresql.conf" <<EOF
port = $port
listen_addresses = 'localhost'
unix_socket_directories = ''
shared_preload_libraries = 'pg_tviews'
extension_control_path = '$work/pkg/share:\$system'
dynamic_library_path = '$work/pkg/lib:\$libdir'
max_prepared_transactions = 10
max_wal_size = 1GB
EOF
# LLVM_PROFILE_FILE (exported by show-env) reaches every backend through the postmaster.
"$bindir/pg_ctl" -D "$work/data" -l "$work/server.log" -w start >/dev/null
trap '"$bindir/pg_ctl" -D "$work/data" -m fast -w stop >/dev/null 2>&1 || true' EXIT

export PGHOST=localhost PGPORT="$port" PGUSER=postgres PGTZ=UTC
PATH="$bindir:$PATH" REGRESS_NO_SKIP=1 ./test/run_regression_tests.sh > "$work/regress.log" 2>&1 \
  || { tail -20 "$work/regress.log"; exit 1; }
PG_CONFIG="$PG_CONFIG" ./test/isolation/run.sh > "$work/isolation.log" 2>&1 \
  || { tail -20 "$work/isolation.log"; exit 1; }
"$bindir/pg_ctl" -D "$work/data" -m fast -w stop >/dev/null
trap - EXIT

# The unit tests add their own profiles to the same directory.
RUSTFLAGS="${RUSTFLAGS:-} ${UNIT_LINK_FLAGS:-} -C link-arg=-Wl,--unresolved-symbols=ignore-all" \
  cargo test --release --lib --no-default-features --features pg18 >/dev/null
cargo llvm-cov report --release --summary-only > "$out/summary.txt"
cargo llvm-cov report --release --lcov --output-path "$out/lcov.info"
grep -E '^TOTAL' "$out/summary.txt"
if [[ -n "${FAIL_UNDER_LINES:-}" ]]; then
  cargo llvm-cov report --release --summary-only --fail-under-lines "$FAIL_UNDER_LINES" >/dev/null
fi

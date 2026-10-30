#!/usr/bin/env bash
# Concurrent differential fuzzer: N pgbench clients write random transactions to
# the base tables of every TVIEW shape of the differential harness and of #263's
# crossed fan-outs, at READ COMMITTED, REPEATABLE READ and SERIALIZABLE, with
# subtransactions that commit or roll back; a checker compares every TVIEW with
# its backing view in REPEATABLE READ snapshots throughout, and once more at the
# end. Every committed snapshot must be fresh.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres \
#     ./test/concurrency/fuzz/run.sh [SECONDS] [SEEDS] [CLIENTS] [MODE]
#
# SECONDS per seed (default 60), SEEDS a space-separated list (default "1"),
# CLIENTS (default 8). MODE:
#   mixed   (default) any statement; 40001 and 40P01 are allowed, any other error
#           a client hits fails the run. LEVELS says at which isolation levels:
#           rc (READ COMMITTED only), rr (6 in 10 READ COMMITTED, the rest
#           REPEATABLE READ) or all (default: 6 READ COMMITTED, 3 REPEATABLE READ,
#           1 SERIALIZABLE in 10). MIX is the percentage of statements drawn from
#           the differential harness, the rest from #263's schema (default 50).
#   values  autocommit single-row writes to columns no TVIEW joins, filters or
#           groups by, at READ COMMITTED: no transaction may fail at all (#263)
#
# PG_CONFIG names pgbench's pg_config (default the one on PATH). KEEP=1 keeps the
# database of a failed seed (named in the output) for inspection.

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER
PG_CONFIG="${PG_CONFIG:-pg_config}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
seconds="${1:-60}"
seeds="${2:-1}"
clients="${3:-8}"
mode="${4:-mixed}"
case "$mode" in
  mixed) script="$here/txn.pgb" ;;
  values) script="$here/values.pgb" ;;
  *) echo "ERROR: mode is mixed or values, not $mode"; exit 2 ;;
esac
case "${LEVELS:-all}" in
  rc) levels=(-D rc=10 -D rr=10) ;;
  rr) levels=(-D rc=6 -D rr=10) ;;
  all) levels=(-D rc=6 -D rr=9) ;;
  *) echo "ERROR: LEVELS is rc, rr or all"; exit 2 ;;
esac
pgbench="$("$PG_CONFIG" --bindir)/pgbench"
db="pg_tviews_fuzz_$$"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

failed=0
for seed in $seeds; do
  psql -d postgres -qc "DROP DATABASE IF EXISTS $db" -c "CREATE DATABASE $db" >/dev/null || exit 2
  psql -d postgres -qc "ALTER DATABASE $db SET search_path = \"\$user\", public, tviews" >/dev/null
  if ! psql -d "$db" -X -q -v ON_ERROR_STOP=1 -v xfail= \
         -f "$here/../../sql/differential/schema.sql" -f "$here/fuzz.sql" >"$work/load.log" 2>&1; then
    echo "ERROR: could not load the schema:"; grep -m3 ERROR "$work/load.log"; exit 2
  fi

  "$pgbench" -n -M simple -c "$clients" -j "$clients" -T "$seconds" --random-seed="$seed" \
    --max-tries=1 --failures-detailed --verbose-errors "${levels[@]}" -D harness="${MIX:-50}" -f "$script" "$db" >"$work/pgbench.log" 2>&1 &
  bench=$!
  checks=0
  : >"$work/stale.log"
  while kill -0 "$bench" 2>/dev/null; do
    diff="$(psql -d "$db" -AtX -q -c "BEGIN ISOLATION LEVEL REPEATABLE READ" \
              -c "SELECT fuzz_check(true)" -c "COMMIT" 2>&1)"
    checks=$((checks + 1))
    [[ -z "$diff" ]] || echo "check $checks:$diff" >>"$work/stale.log"
  done
  wait "$bench"
  final="$(psql -d "$db" -AtX -c "SELECT fuzz_check(false)" 2>&1)"
  [[ -z "$final" ]] || echo "final check:$final" >>"$work/stale.log"

  log="$work/pgbench.log"
  number() { sed -n "s/^number of $1: \([0-9]*\).*/\1/p" "$log" | head -1; }
  done_txns=$(number "transactions actually processed")
  failures=$(number "failed transactions")
  serialization=$(number "serialization failures")
  deadlocks=$(number "deadlock failures")
  aborted=$(grep -cE "^client [0-9]+ aborted|aborted in command" "$log")
  verdict=PASS
  if [[ -s "$work/stale.log" ]]; then
    verdict="FAIL (stale)"
  elif [[ "$aborted" -gt 0 || -z "$done_txns" ]]; then
    verdict="FAIL (client error)"
  elif [[ "$mode" == values && "${failures:-1}" -ne 0 ]]; then
    verdict="FAIL (a value-only write failed)"
  fi
  echo "$verdict  seed $seed: ${done_txns:-?} transactions, ${failures:-?} failed" \
       "(${serialization:-?} serialization, ${deadlocks:-?} deadlocks), $checks checks"
  if [[ "$verdict" != PASS ]]; then
    failed=$((failed + 1))
    if [[ "${KEEP:-}" == 1 ]]; then
      kept="${db}_seed$seed"
      psql -d postgres -qc "ALTER DATABASE $db RENAME TO $kept" && echo "      kept database $kept"
    fi
    head -20 "$work/stale.log"
    grep -E "aborted|ERROR" "$log" | grep -vE "deadlock detected|could not serialize" | head -10
    [[ "$mode" == values ]] && grep -E "ERROR" "$log" | head -5
  fi
done
psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1
[[ "$failed" -eq 0 ]]

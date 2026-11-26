#!/usr/bin/env bash
# Concurrent differential fuzzer: N pgbench clients write random transactions to
# the base tables of every TVIEW shape of the differential harness and of #263's
# crossed fan-outs, at READ COMMITTED, with subtransactions that commit or roll
# back; one transaction in ten runs at REPEATABLE READ or SERIALIZABLE, where
# every write pg_tviews maintains must be refused with 0A000. A checker compares
# every TVIEW with its backing view in REPEATABLE READ snapshots throughout, and
# once more at the end. Every committed snapshot must be fresh.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres \
#     ./test/concurrency/fuzz/run.sh [SECONDS] [SEEDS] [CLIENTS] [MODE]
#
# SECONDS per seed (default 60), SEEDS a space-separated list (default "1"),
# CLIENTS (default 8). MODE:
#   mixed   (default) any statement; 40001 and 40P01 are allowed, any other error
#           a client hits fails the run. MIX is the percentage of statements drawn
#           from the differential harness, the rest from #263's schema (default 50).
#   values  autocommit single-row writes to columns no TVIEW joins, filters or
#           groups by, at READ COMMITTED: no transaction may fail at all (#263)
#
# MODES (test/lib/modes.sh) runs every seed once per mode, e.g. MODES="$MODES_ALL"
# for the patch and cache switches off.
#
# OWNER: who owns the tables and TVIEWs. `role` (default) a role made for the
# run, neither superuser nor the writers, owning the database: the owner frame,
# the guards and row security act as they do in production. `superuser`: the
# connecting superuser, as before.
#
# In mixed mode, with a test-hooks build, a prober runs transactions whose flushes
# are skipped (pg_tviews.test_skip_flush): each must fail its commit with the
# queued work named (I2), or have queued nothing; one that commits with work left
# leaves a TVIEW stale, which the checker reports.
#
# PG_CONFIG names pgbench's pg_config (default the one on PATH). KEEP=1 keeps the
# database of a failed seed (named in the output) for inspection. EXTRA_PGOPTIONS
# is added to every client's PGOPTIONS (e.g. "-c pg_tviews.test_diag_21000=on").

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER
PG_CONFIG="${PG_CONFIG:-pg_config}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../../lib/modes.sh
source "$here/../../lib/modes.sh"
check_modes
seconds="${1:-60}"
seeds="${2:-1}"
clients="${3:-8}"
kind="${4:-mixed}"
case "$kind" in
  mixed) script="$here/txn.pgb" ;;
  values) script="$here/values.pgb" ;;
  *) echo "ERROR: mode is mixed or values, not $kind"; exit 2 ;;
esac
pgbench="$("$PG_CONFIG" --bindir)/pgbench"
db="pg_tviews_fuzz_$$"
owner="tviews_fuzz_owner_$$"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

failed=0
if [[ "${OWNER:-role}" == role ]]; then
  psql -d postgres -qc "CREATE ROLE $owner" >/dev/null || exit 2
fi
for mode in $MODES; do
  PGOPTIONS="$(mode_options "$mode") ${EXTRA_PGOPTIONS:-}"
  export PGOPTIONS
  for seed in $seeds; do
    psql -d postgres -qc "DROP DATABASE IF EXISTS $db" -c "CREATE DATABASE $db" >/dev/null || exit 2
    psql -d postgres -qc "ALTER DATABASE $db SET search_path = \"\$user\", public, tviews" >/dev/null
    as_owner=()
    if [[ "${OWNER:-role}" == role ]]; then
      psql -d postgres -qc "ALTER DATABASE $db OWNER TO $owner" >/dev/null || exit 2
      psql -d "$db" -X -q -c "CREATE EXTENSION IF NOT EXISTS jsonb_delta" \
           -c "CREATE EXTENSION IF NOT EXISTS pg_tviews" -c "ALTER SCHEMA public OWNER TO $owner" >/dev/null || exit 2
      as_owner=(-c "SET ROLE $owner")
    fi
    if ! psql -d "$db" -X -q -v ON_ERROR_STOP=1 -v xfail= "${as_owner[@]}" \
           -f "$here/../../sql/differential/schema.sql" -f "$here/fuzz.sql" >"$work/load.log" 2>&1; then
      echo "ERROR: could not load the schema:"; grep -m3 ERROR "$work/load.log"; exit 2
    fi

    "$pgbench" -n -M simple -c "$clients" -j "$clients" -T "$seconds" --random-seed="$seed" \
      --max-tries=1 --failures-detailed --verbose-errors -D harness="${MIX:-50}" -f "$script" "$db" >"$work/pgbench.log" 2>&1 &
    bench=$!
    : >"$work/probe.log"
    probe=
    hooks="$(psql -d "$db" -AtX -c "SELECT current_setting('pg_tviews.test_skip_flush', true) IS NOT NULL")"
    if [[ "$kind" == mixed && "$hooks" == t ]]; then
      (
        # Every other probe is one statement (fuzz_refused runs one, which nothing
        # refuses at READ COMMITTED): a transaction of one fan-out-only write is
        # the case the commit check once missed.
        probes=0
        while kill -0 "$bench" 2>/dev/null; do
          probes=$((probes + 1))
          fn=fuzz_txn; ((probes % 2 == 0)) && fn=fuzz_refused
          out="$(psql -d "$db" -AtX -q -c "BEGIN" -c "SET LOCAL pg_tviews.test_skip_flush = on" \
                   -c "SELECT $fn($((RANDOM * 32768 + RANDOM)), ${MIX:-50})" \
                   -c "SELECT 'pending ' || (tviews.pg_tviews_show_queue()->>'pending_work')" \
                   -c "COMMIT" 2>&1)"
          pending="$(sed -n 's/^pending //p' <<<"$out")"
          if [[ "$out" == *"queued refreshes for"* ]]; then
            echo refused
          elif [[ "$out" == *"deadlock detected"* || "$out" == *"could not serialize"* ]]; then
            echo retry
          elif [[ "$out" == *ERROR* || "$out" == *FATAL* ]]; then
            echo "error: $(grep -m1 -E 'ERROR|FATAL' <<<"$out")"
          elif [[ "${pending:-0}" != 0 ]]; then
            echo "error: committed with $pending pieces of work queued (I2)"
          else
            echo committed
          fi
        done >>"$work/probe.log"
      ) &
      probe=$!
    fi
    checks=0
    : >"$work/stale.log"
    while kill -0 "$bench" 2>/dev/null; do
      diff="$(psql -d "$db" -AtX -q -c "BEGIN ISOLATION LEVEL REPEATABLE READ" \
                -c "SELECT fuzz_check(true)" -c "COMMIT" 2>&1)"
      checks=$((checks + 1))
      [[ -z "$diff" ]] || echo "check $checks:$diff" >>"$work/stale.log"
    done
    wait "$bench"
    [[ -n "$probe" ]] && wait "$probe"
    final="$(psql -d "$db" -AtX -c "SELECT fuzz_check(false)" 2>&1)"
    [[ -z "$final" ]] || echo "final check:$final" >>"$work/stale.log"

    log="$work/pgbench.log"
    number() { sed -n "s/^number of $1: \([0-9]*\).*/\1/p" "$log" | head -1; }
    done_txns=$(number "transactions actually processed")
    failures=$(number "failed transactions")
    serialization=$(number "serialization failures")
    deadlocks=$(number "deadlock failures")
    aborted=$(grep -cE "^client [0-9]+ aborted|aborted in command" "$log")
    probed="no prober"
    if [[ -n "$probe" ]]; then
      probed="unflushed: $(grep -c '^refused' "$work/probe.log") refused, $(grep -c '^committed' "$work/probe.log") committed with nothing queued"
    fi
    verdict=PASS
    if [[ -s "$work/stale.log" ]]; then
      verdict="FAIL (stale)"
    elif grep -q '^error' "$work/probe.log"; then
      verdict="FAIL (prober error)"
    elif [[ "$aborted" -gt 0 || -z "$done_txns" ]]; then
      verdict="FAIL (client error)"
    elif [[ "$kind" == values && "${failures:-1}" -ne 0 ]]; then
      verdict="FAIL (a value-only write failed)"
    fi
    # I4: each deadlock's class (fuzz_deadlock_class); untagged ones failed in the
    # flush at the end of the outer statement.
    classes="$(grep -E "deadlock detected" "$log" | grep -v "^ *DETAIL" |
               sed -n 's/.*deadlock detected \[\([^]]*\)\].*/\1/p; t; /deadlock detected/s/.*/flush at statement end/p' |
               sort | uniq -c | sort -rn | awk '{n=$1; $1=""; printf "%s%s:%s", sep, n, $0; sep=";"}')"
    echo "$verdict  seed $seed [$mode]: ${done_txns:-?} transactions, ${failures:-?} failed" \
         "(${serialization:-?} serialization, ${deadlocks:-?} deadlocks), $checks checks; $probed"
    [[ -n "$classes" ]] && echo "      deadlock errors by class (I4):$classes"
    if [[ "$verdict" != PASS ]]; then
      failed=$((failed + 1))
      if [[ "${KEEP:-}" == 1 ]]; then
        kept="${db}_seed${seed}_$(tr -c "a-z0-9\n" _ <<<"$mode")"
        psql -d postgres -qc "ALTER DATABASE $db RENAME TO $kept" && echo "      kept database $kept"
      fi
      head -20 "$work/stale.log"
      grep -m5 '^error' "$work/probe.log"
      grep -E "aborted|ERROR" "$log" | grep -vE "deadlock detected|could not serialize" | head -10
      [[ "$kind" == values ]] && grep -E "ERROR" "$log" | head -5
    fi
  done
done
unset PGOPTIONS
psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1
# A kept database still belongs to the role: it stays until that database is dropped.
psql -d postgres -qc "DROP ROLE IF EXISTS $owner" >/dev/null 2>&1 \
  || echo "kept role $owner: it owns the kept databases"
[[ "$failed" -eq 0 ]]

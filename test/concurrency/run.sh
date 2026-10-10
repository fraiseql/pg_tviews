#!/usr/bin/env bash
# Concurrency harness: pgbench workloads against TVIEWs, checking every TVIEW
# against its backing view afterwards. Each run uses a fresh database.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/concurrency/run.sh [workload ...]
#
# Workloads (default: uniform hot single):
#   uniform  16 clients, 80% post inserts / 20% renames over 1,000 users
#   hot      the same over 10 users
#   bulk     uniform inserts over 20,000 users while one transaction renames them all
#   single   one client, the uniform mix: the per-statement cost
#   stress   uniform over orgs too (post -> user -> org): inserts, renames of
#            users and orgs, users moving between orgs, and a rename of every
#            user in one transaction every 30 s; 40P01 counts as failed
#
# Environment: PG_CONFIG (pgbench's pg_config, default the one on PATH),
# CLIENTS (16), DURATION seconds per run (20), RUNS per workload (1; the median
# tps is printed when more), ISOLATION (default_transaction_isolation, default
# read committed), MAX_TRIES (pgbench retries of 40001/40P01, default 1).
#
# One line per run: tps, average latency, failed transactions (after MAX_TRIES),
# transactions retried, aborted clients and stale rows. Exits non-zero when a run left a stale
# row or a client aborted.

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER
PG_CONFIG="${PG_CONFIG:-pg_config}"
CLIENTS="${CLIENTS:-16}"
DURATION="${DURATION:-20}"
RUNS="${RUNS:-1}"
ISOLATION="${ISOLATION:-read committed}"
MAX_TRIES="${MAX_TRIES:-1}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pgbench="$("$PG_CONFIG" --bindir)/pgbench"
db="pg_tviews_concurrency_$$"
work="$(mktemp -d)"
trap 'psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1; rm -rf "$work"' EXIT
status=0

# fresh_db <users> [extra.sql]: a new database with the harness schema.
fresh_db() {
  psql -d postgres -qc "DROP DATABASE IF EXISTS $db" -c "CREATE DATABASE $db" >/dev/null || exit 2
  psql -d "$db" -qX -v ON_ERROR_STOP=1 -v users="$1" -f "$here/setup.sql" >/dev/null || exit 2
  [[ -z "${2:-}" ]] || psql -d "$db" -qX -v ON_ERROR_STOP=1 -f "$here/$2" >/dev/null || exit 2
}

# bench <clients> <hot users> <log>: the insert/rename mix for DURATION seconds.
bench() {
  PGOPTIONS="-c default_transaction_isolation=${ISOLATION// /\\ }" \
    "$pgbench" -n -c "$1" -j "$(( $1 < 4 ? $1 : 4 ))" -T "$DURATION" --max-tries="$MAX_TRIES" \
    -D hot="$2" -f "$here/insert.pgb@8" -f "$here/rename.pgb@2" "$db" >"$3" 2>&1
}

# report <workload> <log> [extra]: one result line; records a failure in status.
report() {
  local tps lat failed retried aborted stale
  tps=$(sed -n 's/^tps = \([0-9.]*\).*/\1/p' "$2")
  lat=$(sed -n 's/^latency average = \([0-9.]*\) ms/\1/p' "$2")
  failed=$(sed -n 's/^number of failed transactions: \([0-9]*\).*/\1/p' "$2")
  retried=$(sed -n 's/^number of transactions retried: \([0-9]*\).*/\1/p' "$2")
  aborted=$(grep -c "aborted in command" "$2")
  stale=$(psql -d "$db" -AtX -f "$here/stale.sql")
  if [[ "$1" == stress ]]; then
    stale=$(( stale + $(psql -d "$db" -AtX -c "SELECT count(*) FROM tv_feed t
      FULL JOIN tviews.public__tv_feed v USING (pk_feed) WHERE t.data IS DISTINCT FROM v.data") ))
  fi
  printf '%-8s tps=%-8s lat_ms=%-7s failed=%s retried=%s aborted=%s stale=%s %s\n' \
    "$1" "${tps:-?}" "${lat:-?}" "${failed:-?}" "${retried:-0}" "$aborted" "${stale:-?}" "${3:-}"
  grep -m3 -E "ERROR|aborted" "$2" | sed 's/^/    /'
  [[ "$stale" == "0" && "$aborted" == "0" && -n "$tps" ]] || status=1
  echo "${tps:-0}" >>"$work/$1.tps"
}

run_workload() {
  local log="$work/$1.log"
  case "$1" in
    uniform) fresh_db 1000; bench "$CLIENTS" 1000 "$log"; report "$1" "$log" ;;
    hot) fresh_db 1000; bench "$CLIENTS" 10 "$log"; report "$1" "$log" ;;
    single) fresh_db 1000; bench 1 1000 "$log"; report "$1" "$log" ;;
    bulk)
      fresh_db 20000
      bench "$CLIENTS" 20000 "$log" &
      local pid=$! start end
      sleep "$(( DURATION / 4 ))"
      start=$(date +%s%N)
      psql -d "$db" -qX -v ON_ERROR_STOP=1 -c "SET pg_tviews.max_queue_size = 100000" \
        -c "UPDATE tb_user SET name = name || '!'" >"$work/bulk.err" 2>&1 || status=1
      end=$(date +%s%N)
      wait "$pid"
      report "$1" "$log" "bulk_ms=$(( (end - start) / 1000000 ))"
      sed 's/^/    /' "$work/bulk.err"
      ;;
    stress)
      fresh_db 1000 stress.sql
      PGOPTIONS="-c default_transaction_isolation=${ISOLATION// /\\ }" \
        "$pgbench" -n -c "$CLIENTS" -j 4 -T "$DURATION" --max-tries="$MAX_TRIES" \
        --failures-detailed -D hot=1000 -f "$here/insert.pgb@6" -f "$here/rename.pgb@2" \
        -f "$here/org.pgb@1" -f "$here/move.pgb@1" "$db" >"$log" 2>&1 &
      local pid=$! bulks=0
      while kill -0 "$pid" 2>/dev/null; do
        sleep 30
        kill -0 "$pid" 2>/dev/null || break
        psql -d "$db" -qX -c "SET pg_tviews.max_queue_size = 100000" -c "UPDATE tb_user
          SET name = name || '!'" >>"$work/bulk.err" 2>&1 && bulks=$((bulks + 1))
      done
      wait "$pid"
      report "$1" "$log" "bulks=$bulks deadlocks=$(sed -n 's/^.*deadlock failures: \([0-9]*\).*/\1/p' "$log" | head -1)"
      grep -v "^$" "$work/bulk.err" 2>/dev/null | head -3 | sed 's/^/    /'
      ;;
    *) echo "unknown workload: $1"; exit 2 ;;
  esac
}

[[ $# -gt 0 ]] || set -- uniform hot single
for workload in "$@"; do
  for _ in $(seq "$RUNS"); do run_workload "$workload"; done
  if [[ "$RUNS" -gt 1 ]]; then
    echo "$workload median tps=$(sort -n "$work/$workload.tps" | sed -n "$(( (RUNS + 1) / 2 ))p")"
  fi
done
exit $status

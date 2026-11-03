#!/usr/bin/env bash
# Statements and plans per write, pg_tviews' triggers and flush included: what a
# change to how triggers dispatch must leave alone. Builds are compared write by
# write on clusters that differ only by the pg_tviews they load.
#
# Usage: PGHOST=… PGUSER=… ./test/bench/write_path/counts.sh <label>=<port> ... -- \
#          [transactions] [write ...]
# Each cluster needs pg_stat_statements in shared_preload_libraries, with
# pg_stat_statements.track = all and pg_stat_statements.track_planning = on.
# Prints write,label,statements,plans per transaction (one client, each write on a
# fresh copy of the seeded schema), then one line per write: same, or differs.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$here/lib.sh"
builds=()
while [[ $# -gt 0 && "$1" != -- ]]; do builds+=("$1"); shift; done
[[ $# -gt 0 ]] && shift
txs="${1:-50}"; shift $(( $# > 0 ? 1 : 0 ))
writes=("$@")
[[ ${#writes[@]} -eq 0 ]] && writes=(bio username title comment_insert bio_bulk100 untracked_insert username_two_hop username_array)
[[ ${#builds[@]} -lt 1 ]] && { echo "usage: $0 <label>=<port> ... -- [transactions] [write ...]" >&2; exit 2; }
tpl=pg_tviews_write_bench; db=pg_tviews_write_bench_run

for b in "${builds[@]}"; do PGPORT="${b#*=}" seed_template "$tpl" "$here/schema.sql"; done
out="$(mktemp)"; trap 'rm -f "$out"' EXIT
echo "write,label,statements,plans" | tee "$out"
for w in "${writes[@]}"; do
  for b in "${builds[@]}"; do
    export PGPORT="${b#*=}"
    fresh_copy "$tpl" "$db"
    psql -X -q -d "$db" -c "CREATE EXTENSION IF NOT EXISTS pg_stat_statements" \
         -c "SELECT pg_stat_statements_reset()" >/dev/null
    pgbench -n -c 1 -t "$txs" -f "$here/writes/$w.sql" "$db" >/dev/null 2>&1 \
      || { echo "pgbench failed: $w on ${b%%=*}" >&2; exit 1; }
    psql -X -At -F, -d "$db" -c "
      SELECT '$w', '${b%%=*}', round(sum(calls)::numeric / $txs, 2), round(sum(plans)::numeric / $txs, 2)
      FROM pg_stat_statements
      WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database())
        AND query NOT LIKE '%pg_stat_statements%'" | tee -a "$out"
  done
done
echo
for w in "${writes[@]}"; do
  if [[ "$(grep "^$w," "$out" | cut -d, -f3,4 | sort -u | wc -l)" == 1 ]]; then
    echo "$w,same"
  else
    echo "$w,differs"
  fi
done

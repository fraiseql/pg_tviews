#!/usr/bin/env bash
# Refreshes per mutation, fraiseql-shaped (the next wave's mutation deferral sizes
# its gain on this): each mutation function of mutations.sql written per statement
# (the way such functions are often written) and once per row, called as one outer
# SELECT. For each: ms per mutation (pgbench, 1 client), and per mutation the TVIEW
# rows refreshed (tviews.stats: view recomputes, patches applied, rows written).
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/write_path/mutations.sh [seconds]
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$here/lib.sh"
secs="${1:-10}"
tpl=pg_tviews_mutation_bench; db=pg_tviews_mutation_bench_run
if ! psql -X -d "$tpl" -tAc "SELECT 1 FROM mb.tv_allocation LIMIT 1" >/dev/null 2>&1; then
  psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $tpl" -c "CREATE DATABASE $tpl"
  psql -X -q -v ON_ERROR_STOP=1 -d "$tpl" -f "$here/mutations.sql" >/dev/null
fi
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
OPTS="${OPTS:--c synchronous_commit=off}"
echo "mutation,variant,ms,recomputes,patches,rows_written"
for m in update_location create_location log_events; do
  for v in per_statement once; do
    fresh_copy "$tpl" "$db"
    case "$m" in
      create_location) call="SELECT mb.${m}_${v}();" ;;
      *) printf '\\set p random(1, 200)\n' >"$work/f.sql"; call="SELECT mb.${m}_${v}(:p);" ;;
    esac
    [[ "$m" == create_location ]] && : >"$work/f.sql"
    echo "$call" >>"$work/f.sql"
    # Counters of 50 calls, then the time of a timed run.
    psql -X -q -d "$db" -c "SELECT tviews.pg_tviews_reset_stats_all()" >/dev/null
    for _ in $(seq 50); do
      psql -X -q -d "$db" -c "${call//:p/$(( RANDOM % 200 + 1 ))}" >/dev/null
    done
    counts="$(psql -X -At -F, -d "$db" -c "SELECT round(sum(view_recomputes) / 50.0, 1),
                round(sum(patch_applied) / 50.0, 1), round(sum(rows_written) / 50.0, 1)
              FROM tviews.stats")"
    read -r _ ms _ < <(measure "$db" "$work/f.sql" "$secs" 1 "$OPTS")
    echo "$m,$v,$ms,$counts"
  done
done

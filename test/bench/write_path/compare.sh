#!/usr/bin/env bash
# Compare builds on the write path: the same writes, interleaved build by build, on
# clusters that differ only by the pg_tviews they load, each round started only on a
# quiet machine.
#
# Usage: PGHOST=… PGUSER=… ./test/bench/write_path/compare.sh <label>=<port> ... -- \
#          [rounds] [seconds] [clients ...] [-- write ...]
#   LOAD_MAX=4        start a run only when the 1-minute load average is below this
#   LOAD_DISCARD=8    discard a run whose machine was at this load or more when it ended,
#                     plus half its clients (the run's own load)
#   QUIET_TIMEOUT=7200  give up (exit 3) after waiting this many seconds for a quiet machine
# Prints one CSV line per run: round,write,clients,label,tps,ms,wal_B,load_start,load_end,kept
# Then, per write and client count, each build's median tps over the kept runs.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$here/lib.sh"
builds=()
while [[ $# -gt 0 && "$1" != -- ]]; do builds+=("$1"); shift; done
[[ $# -gt 0 ]] && shift
rounds="${1:-3}"; secs="${2:-15}"; shift $(( $# > 2 ? 2 : $# ))
clients=()
while [[ $# -gt 0 && "$1" != -- ]]; do clients+=("$1"); shift; done
[[ $# -gt 0 ]] && shift
writes=("$@")
[[ ${#clients[@]} -eq 0 ]] && clients=(1 16)
[[ ${#writes[@]} -eq 0 ]] && writes=(bio username title comment_insert bio_bulk100 untracked_insert)
[[ ${#builds[@]} -lt 2 ]] && { echo "usage: $0 <label>=<port> <label>=<port> ... -- …" >&2; exit 2; }
tpl=pg_tviews_write_bench; db=pg_tviews_write_bench_run
load_max="${LOAD_MAX:-4}"; load_discard="${LOAD_DISCARD:-8}"; quiet_timeout="${QUIET_TIMEOUT:-7200}"

load() { cut -d' ' -f1 /proc/loadavg; }
below() { awk -v a="$1" -v b="$2" 'BEGIN{exit !(a < b)}'; }
wait_quiet() {
  local waited=0
  until below "$(load)" "$load_max"; do
    (( waited >= quiet_timeout )) && { echo "no quiet machine in ${quiet_timeout}s (load $(load))" >&2; exit 3; }
    sleep 30; waited=$(( waited + 30 ))
  done
}

for b in "${builds[@]}"; do PGPORT="${b#*=}" seed_template "$tpl" "$here/schema.sql"; done
out="$(mktemp)"; trap 'rm -f "$out"' EXIT
echo "round,write,clients,label,tps,ms,wal_B,load_start,load_end,kept" | tee "$out"
for (( r = 1; r <= rounds; r++ )); do
  for w in "${writes[@]}"; do
    for c in "${clients[@]}"; do
      # Alternate which build runs first, so neither always follows the other.
      order=("${builds[@]}")
      (( r % 2 == 0 )) && mapfile -t order < <(printf '%s\n' "${builds[@]}" | tac)
      for b in "${order[@]}"; do
        export PGPORT="${b#*=}"
        fresh_copy "$tpl" "$db"
        wait_quiet; l0="$(load)"
        read -r tps ms wal < <(measure "$db" "$here/writes/$w.sql" "$secs" "$c")
        l1="$(load)"; kept=yes; below "$l1" "$(( load_discard + c / 2 ))" || kept=no
        echo "$r,$w,$c,${b%%=*},$tps,$ms,$wal,$l0,$l1,$kept" | tee -a "$out"
      done
    done
  done
done
for b in "${builds[@]}"; do PGPORT="${b#*=}" psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $db"; done
echo; echo "write,clients,label,median_tps,runs"
awk -F, 'NR > 1 && $10 == "yes" { v[$2 "," $3 "," $4] = v[$2 "," $3 "," $4] " " $5 }
  END {
    for (k in v) {
      n = split(substr(v[k], 2), a, " "); asort(a)
      m = (n % 2) ? a[(n + 1) / 2] : (a[n / 2] + a[n / 2 + 1]) / 2
      print k "," m "," n
    }
  }' "$out" | sort

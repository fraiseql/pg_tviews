#!/usr/bin/env bash
# Fan-out scaling (#233): what one write costs when N TVIEW rows embed the row it
# changes. `UPDATE tb_cat SET name = …` on a category embedded in N items, for each N,
# with pg_tviews patching the items (patch), recomputing them from their view
# (recompute), and with triggers off (plain). Each cell runs on a fresh copy of the
# seeded database. Prints ms and WAL bytes per write.
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/write_path/fanout.sh [seconds] [doc_bytes]
#   FANOUTS="1 10 100 1000 10000"   the fan-outs (default)
#   MODES="patch recompute plain"   the modes (default)
# doc_bytes: ~1000 (default) keeps documents inline; 4000 stores them TOASTed.
# Mind the WAL: 4 KB documents at N = 10,000 write ~50 MB per write.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$here/lib.sh"
secs="${1:-10}"; doc_bytes="${2:-1000}"
read -r -a fanouts <<<"${FANOUTS:-1 10 100 1000 10000}"
read -r -a modes <<<"${MODES:-patch recompute plain}"
list="$(IFS=,; echo "${fanouts[*]}")"
tpl="pg_tviews_fanout_bench_${doc_bytes}_${list//,/_}"; db=pg_tviews_fanout_bench_run
if ! psql -d "$tpl" -tAc "SELECT 1 FROM fan.tv_item LIMIT 1" >/dev/null 2>&1; then
  psql -q -d postgres -c "DROP DATABASE IF EXISTS $tpl" -c "CREATE DATABASE $tpl"
  psql -q -X -v ON_ERROR_STOP=1 -v fanouts="$list" -v doc_bytes="$doc_bytes" -d "$tpl" \
       -c "SET pg_tviews.max_queue_size = 2000000" -f "$here/fanout.sql" >/dev/null
fi
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"; psql -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT
echo "documents: $(psql -X -At -d "$tpl" -c "SELECT round(avg(pg_column_size(data))) || ' B, ' || count(*) || ' items' FROM fan.tv_item")"
printf '%7s %-9s %10s %12s\n' fanout mode ms/write wal_B/write
for n in "${fanouts[@]}"; do
  read -r lo hi < <(psql -X -At -F ' ' -d "$tpl" -c "SELECT min(pk_cat), max(pk_cat) FROM fan.tb_cat WHERE fanout = $n")
  printf '\\set c random(%d, %d)\nUPDATE fan.tb_cat SET name = %s || random() WHERE pk_cat = :c;\n' \
    "$lo" "$hi" "'n'" > "$tmp/w.sql"
  for mode in "${modes[@]}"; do
    case "$mode" in
      patch) opts="";;
      recompute) opts="-c pg_tviews.direct_patch_enabled=off";;
      plain) opts="-c session_replication_role=replica";;
      *) echo "unknown mode $mode" >&2; exit 2;;
    esac
    fresh_copy "$tpl" "$db"
    read -r _ ms wal < <(measure "$db" "$tmp/w.sql" "$secs" 1 "$opts")
    printf '%7s %-9s %10s %12s\n' "$n" "$mode" "$ms" "$wal"
  done
done

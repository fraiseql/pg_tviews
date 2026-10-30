#!/usr/bin/env bash
# #224 write-path benchmark: tps and WAL per transaction of each write with pg_tviews,
# and of the same write with triggers off (session_replication_role = replica) as
# the plain baseline. Each run starts on a fresh copy of the seeded database.
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/write_path/run.sh [seconds] [clients] [write ...]
#   RECOMPUTE=1   refresh by recomputing rows from their views (direct_patch_enabled = off)
# Seeds (once) the template database pg_tviews_write_bench. Not run in CI. Time only a
# release build (`cargo pgrx package`) on a stock server: a pgrx dev build is
# debug + cassert and several times slower.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$here/lib.sh"
secs="${1:-10}"; clients="${2:-1}"; shift $(( $# > 2 ? 2 : $# ))
writes=("$@"); [[ ${#writes[@]} -eq 0 ]] && writes=(bio username title comment_insert bio_bulk100)
tpl=pg_tviews_write_bench; db=pg_tviews_write_bench_run
seed_template "$tpl" "$here/schema.sql"
opts=""; [[ "${RECOMPUTE:-0}" == 1 ]] && opts="-c pg_tviews.direct_patch_enabled=off"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"; psql -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT
printf '%-16s %10s %12s %10s %12s %7s\n' write tps wal_B/txn plain_tps plain_wal ratio
for w in "${writes[@]}"; do
  { grep '^[\]' "$here/writes/$w.sql" || true
    echo "BEGIN;"; echo "SET LOCAL session_replication_role = replica;"
    grep -v '^[\]' "$here/writes/$w.sql"; echo "COMMIT;"; } > "$tmp/$w.plain.sql"
  fresh_copy "$tpl" "$db"; read -r a _ aw < <(measure "$db" "$here/writes/$w.sql" "$secs" "$clients" "$opts")
  fresh_copy "$tpl" "$db"; read -r b _ bw < <(measure "$db" "$tmp/$w.plain.sql" "$secs" "$clients")
  printf '%-16s %10s %12s %10s %12s %6sx\n' "$w" "$a" "$aw" "$b" "$bw" \
    "$(awk -v a="$a" -v b="$b" 'BEGIN{printf "%.1f", (a>0)?b/a:0}')"
done

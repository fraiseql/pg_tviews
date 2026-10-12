#!/usr/bin/env bash
# #224 write-path benchmark: tps of each write with pg_tviews, and the same write
# with triggers off (session_replication_role = replica) as the plain baseline.
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/write_path/run.sh [seconds] [clients] [write ...]
# Creates (or reuses) database pg_tviews_write_bench. Not run in CI. Time only a
# release build (`cargo pgrx package`) on a stock server: a pgrx dev build is
# debug + cassert and several times slower.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
secs="${1:-10}"; clients="${2:-1}"; shift $(( $# > 2 ? 2 : $# ))
writes=("$@"); [[ ${#writes[@]} -eq 0 ]] && writes=(bio username title comment_insert bio_bulk100)
db=pg_tviews_write_bench
if ! psql -d "$db" -tAc "SELECT 1 FROM bench.tv_comment LIMIT 1" >/dev/null 2>&1; then
  psql -q -d postgres -c "DROP DATABASE IF EXISTS $db" -c "CREATE DATABASE $db"
  psql -q -X -v ON_ERROR_STOP=1 -d "$db" -c "SET pg_tviews.max_queue_size = 1000000" -f "$here/schema.sql" >/dev/null
fi
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
tps() { pgbench -n -c "$clients" -j "$clients" -T "$secs" -f "$1" "$db" 2>/dev/null \
          | sed -n 's/^tps = \([0-9.]*\).*/\1/p' | cut -d. -f1; }
printf '%-16s %10s %10s %8s\n' write pg_tviews plain ratio
for w in "${writes[@]}"; do
  { echo "BEGIN;"; echo "SET LOCAL session_replication_role = replica;"; } > "$tmp/plain.sql"
  grep '^\\' "$here/writes/$w.sql" > "$tmp/plain_meta.sql" || true
  { cat "$tmp/plain_meta.sql"; cat "$tmp/plain.sql"; grep -v '^\\' "$here/writes/$w.sql"; echo "COMMIT;"; } > "$tmp/$w.plain.sql"
  a=$(tps "$here/writes/$w.sql"); b=$(tps "$tmp/$w.plain.sql")
  printf '%-16s %10s %10s %7sx\n' "$w" "$a" "$b" "$(awk -v a="$a" -v b="$b" 'BEGIN{printf "%.1f", (a>0)?b/a:0}')"
done

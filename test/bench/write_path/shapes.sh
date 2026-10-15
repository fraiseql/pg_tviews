#!/usr/bin/env bash
# #228 shapes: what a username write costs when the name is copied into arrays of
# embedded rows (array) or across two hops (two_hop). For each shape: pg_tviews
# (patch path, falling back to recompute where it can't patch), pg_tviews with the
# patch off (recompute), the hand-written patch (*.manual.sql, triggers off) and
# the plain write. Each cell on a fresh copy of the shape's seeded database.
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/write_path/shapes.sh [seconds] [shape ...]
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$here/lib.sh"
secs="${1:-10}"; shift $(( $# > 0 ? 1 : 0 ))
shapes=("$@"); [[ ${#shapes[@]} -eq 0 ]] && shapes=(array two_hop)
db=pg_tviews_shapes_run
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"; psql -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT
printf '%-8s %-10s %10s %12s\n' shape mode ms/write wal_B/write
for shape in "${shapes[@]}"; do
  tpl="pg_tviews_shapes_$shape"
  if ! psql -d "$tpl" -tAc "SELECT 1 FROM bench.tb_comment LIMIT 1" >/dev/null 2>&1; then
    psql -q -d postgres -c "DROP DATABASE IF EXISTS $tpl" -c "CREATE DATABASE $tpl"
    psql -q -X -v ON_ERROR_STOP=1 -v shape="$shape" -d "$tpl" \
         -c "SET pg_tviews.max_queue_size = 1000000" -f "$here/shapes.sql" >/dev/null
  fi
  w="$here/writes/username_$shape.sql"
  { grep '^[\]' "$w"; echo "BEGIN;"; echo "SET LOCAL session_replication_role = replica;"
    grep -v '^[\]' "$w"; echo "COMMIT;"; } > "$tmp/plain.sql"
  for mode in pg_tviews recompute manual plain; do
    case "$mode" in
      pg_tviews) file="$w"; opts="";;
      recompute) file="$w"; opts="-c pg_tviews.direct_patch_enabled=off";;
      manual) file="$here/writes/username_$shape.manual.sql"; opts="";;
      plain) file="$tmp/plain.sql"; opts="";;
    esac
    fresh_copy "$tpl" "$db"
    read -r _ ms wal < <(measure "$db" "$file" "$secs" 1 "$opts")
    printf '%-8s %-10s %10s %12s\n' "$shape" "$mode" "$ms" "$wal"
  done
done

#!/usr/bin/env bash
# jsonb_delta's patch functions against core jsonb operators, per row (ADR 0233, the
# jsonb_delta dependency). A table of 2,000 documents of a given size, whose `data`
# holds a nested object `o`; each statement patches every row:
#
#   nested  delta: jsonb_smart_patch_nested(data, v, '{o}')
#           core:  jsonb_set(data, '{o}', (data -> 'o') || v)
#   scalar  delta: jsonb_smart_patch_scalar(data, v)
#           core:  data || v
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/write_path/patch.sh [rounds] [doc bytes ...]
# Default sizes: 490 (inline) and 23000 (TOASTed). Prints one CSV line per run
# (round,bytes,shape,impl,us_per_row) from EXPLAIN ANALYZE of a SELECT computing the
# patched documents (no write, so no WAL or heap noise), then the medians.
set -euo pipefail
rounds="${1:-5}"; shift $(( $# > 0 ? 1 : 0 ))
sizes=("$@"); [[ ${#sizes[@]} -eq 0 ]] && sizes=(490 23000)
db=pg_tviews_patch_bench rows=2000
psql -X -q -d postgres -c "SET client_min_messages = warning" -c "DROP DATABASE IF EXISTS $db" \
     -c "CREATE DATABASE $db" >/dev/null
psql -X -q -v ON_ERROR_STOP=1 -d "$db" >/dev/null <<SQL
CREATE EXTENSION jsonb_delta;
SQL
for b in "${sizes[@]}"; do
  # Filler sized so the whole document is about b bytes as jsonb text; random text
  # keeps a large document from compressing below the TOAST threshold.
  psql -X -q -v ON_ERROR_STOP=1 -d "$db" >/dev/null <<SQL
CREATE TABLE doc_$b (k int PRIMARY KEY, data jsonb);
INSERT INTO doc_$b SELECT g, jsonb_build_object('id', g, 'name', 'n' || g,
    'o', jsonb_build_object('a', 'x', 'b', g, 'c', 'y'),
    'fill', (SELECT string_agg(md5(random()::text || i), '') FROM generate_series(1, greatest(1, ($b - 120) / 32)) i))
  FROM generate_series(1, $rows) g;
ANALYZE doc_$b;
SQL
done

declare -A E=(
  [nested,delta]="jsonb_smart_patch_nested(data, '{\"a\": \"z\"}', '{o}')"
  [nested,core]="jsonb_set(data, '{o}', (data -> 'o') || '{\"a\": \"z\"}')"
  [scalar,delta]="jsonb_smart_patch_scalar(data, '{\"name\": \"z\"}')"
  [scalar,core]="data || '{\"name\": \"z\"}'"
)
out="$(mktemp)"; trap 'rm -f "$out"' EXIT
echo "round,bytes,shape,impl,us_per_row"
for (( r = 1; r <= rounds; r++ )); do
  for b in "${sizes[@]}"; do
    for shape in nested scalar; do
      impls=(delta core); (( r % 2 == 0 )) && impls=(core delta)
      for impl in "${impls[@]}"; do
        # pg_column_size() forces each patched document to be built.
        ms="$(psql -X -At -d "$db" -c "EXPLAIN (ANALYZE, TIMING OFF, SUMMARY ON, FORMAT JSON)
              SELECT sum(pg_column_size(${E[$shape,$impl]})) FROM doc_$b" |
              sed -n 's/.*"Execution Time": \([0-9.]*\).*/\1/p')"
        us="$(awk -v ms="$ms" -v n="$rows" 'BEGIN { printf "%.2f", ms * 1000 / n }')"
        echo "$r,$b,$shape,$impl,$us" | tee -a "$out"
      done
    done
  done
done
echo
echo "bytes,shape,impl,median_us_per_row"
awk -F, '{ key = $2 "," $3 "," $4; v[key, ++n[key]] = $5 }
  END { for (key in n) { m = n[key]; for (i = 1; i <= m; i++) a[i] = v[key, i]
          for (i = 1; i <= m; i++) for (j = i + 1; j <= m; j++) if (a[j] < a[i]) { t = a[i]; a[i] = a[j]; a[j] = t }
          printf "%s,%.2f\n", key, (m % 2) ? a[(m + 1) / 2] : (a[m / 2] + a[m / 2 + 1]) / 2 } }' "$out" | sort

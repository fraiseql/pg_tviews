#!/usr/bin/env bash
# One run of the time-refresh workload: the database loaded afresh (schema.sql), 8
# writers running the 40/20/20/20 save mix for DURATION seconds, and the time refresh
# called once AT seconds in, in one of two forms:
#   FORM=one         one call refreshing every time-dependent TVIEW, one transaction
#   FORM=per_tview   one transaction per TVIEW, dependencies first
#   FORM=call        CALL tviews.pg_tviews_time_boundary(): the same, in one procedure
# Per-transaction latencies (pgbench -l), lock waits sampled every 100 ms and
# tviews.stats before and after go to OUT/<label>/; summarize.py reads them. A save or a
# refresh that fails (a deadlock, 40P01) is recorded, not retried.
#
# Usage: FORM=one OUT=out ./run.sh <label>
# Environment: DB (bench_time), SCALE (1), CLIENTS (8), DURATION (600), AT (180),
# PGHOST/PGPORT/PGUSER, PG_BIN (directory of psql and pgbench).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
label="${1:?usage: run.sh <label>}"
DB="${DB:-bench_time}"; SCALE="${SCALE:-1}"; CLIENTS="${CLIENTS:-8}"
DURATION="${DURATION:-600}"; AT="${AT:-180}"; FORM="${FORM:-one}"; OUT="${OUT:-$here/out}"
PG_BIN="${PG_BIN:-}"; psql=("${PG_BIN:+$PG_BIN/}psql" -X -q -v ON_ERROR_STOP=1)
pgbench="${PG_BIN:+$PG_BIN/}pgbench"
dir="$OUT/$label"; mkdir -p "$dir"

"${psql[@]}" -d postgres -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)" -c "CREATE DATABASE $DB"
"${psql[@]}" -d "$DB" -v scale="$SCALE" -f "$here/schema.sql" >/dev/null
"${psql[@]}" -d "$DB" -c "CHECKPOINT"
"${psql[@]}" -d "$DB" -At -c "COPY (SELECT entity, full_refreshes, view_recomputes, rows_written FROM tviews.stats ORDER BY 1) TO STDOUT WITH CSV" > "$dir/stats_before.csv"
# One call before the run, as a warm-up, gives the dependency order of the TVIEWs.
order=$("${psql[@]}" -d "$DB" -At -c "SELECT string_agg(t, ' ' ORDER BY n) FROM tviews.pg_tviews_refresh_time_dependent() WITH ORDINALITY AS r(t, n)")

"${psql[@]}" -d "$DB" -c "CALL bench_sample($((DURATION + 20)))" &
sampler=$!
(cd "$dir" && "$pgbench" -n -c "$CLIENTS" -j "$CLIENTS" -T "$DURATION" -D scale="$SCALE" \
   -f "$here/save_placement.sql@40" -f "$here/save_asset.sql@20" \
   -f "$here/save_site.sql@20" -f "$here/save_addon.sql@20" \
   -l --log-prefix=pgb "$DB" > pgbench.out 2>&1) &
bench=$!

sleep "$AT"
start=$(date +%s.%N)
case "$FORM" in
  one) "${psql[@]}" -d "$DB" -At -c "SELECT count(*) FROM tviews.pg_tviews_refresh_time_dependent()" \
         > "$dir/refresh.out" 2>&1 || echo "FAILED all" >> "$dir/refresh.out" ;;
  per_tview)
    for tv in $order; do
      "${psql[@]}" -d "$DB" -At -c "SELECT tviews.pg_tviews_refresh_time_dependent('$tv')" \
        >> "$dir/refresh.out" 2>&1 || echo "FAILED $tv" >> "$dir/refresh.out"
    done ;;
  call) "${psql[@]}" -d "$DB" -At -c "CALL tviews.pg_tviews_time_boundary()" \
          > "$dir/refresh.out" 2>&1 || echo "FAILED all" >> "$dir/refresh.out" ;;
  *) echo "unknown FORM $FORM" >&2; exit 2 ;;
esac
end=$(date +%s.%N)

wait "$bench"
wait "$sampler" || true
"${psql[@]}" -d "$DB" -At -c "COPY (SELECT entity, full_refreshes, view_recomputes, rows_written FROM tviews.stats ORDER BY 1) TO STDOUT WITH CSV" > "$dir/stats_after.csv"
"${psql[@]}" -d "$DB" -At -c "COPY (SELECT extract(epoch FROM at), locktype, mode, coalesce(relation, ''), coalesce(field4, -1), waiting FROM bench_waits ORDER BY at) TO STDOUT WITH CSV" > "$dir/waits.csv"
printf '{"label": "%s", "form": "%s", "refresh_start": %s, "refresh_end": %s, "duration": %s, "clients": %s, "scale": %s}\n' \
  "$label" "$FORM" "$start" "$end" "$DURATION" "$CLIENTS" "$SCALE" > "$dir/meta.json"
echo "$label: refresh $(grep -c '^FAILED' "$dir/refresh.out" || true) failed, $(python3 -c "print(round($end - $start, 2))") s; $(grep -E '^tps' "$dir/pgbench.out" | head -1)"

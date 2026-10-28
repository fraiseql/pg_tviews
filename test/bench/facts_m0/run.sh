#!/usr/bin/env bash
# ADR 0260 M0: one fact family (day → month → year, and a category × month rollup)
# kept by generated statement triggers, on a database without pg_tviews_facts.
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/facts_m0/run.sh [correctness|isolation|overhead] …
#   correctness [rounds] [statements]  randomized load, verify after each round (default 10 × 100)
#   isolation                          concurrent loads (isolation/specs), via pg_isolation_regress
#                                      (FACTS_M0_LOCK=false generates the family without its lock)
#   overhead [seconds] [clients]       µs and WAL per inserted row, with and without the triggers
# Not run in CI. Run it on a server built without assertions: on an assert-enabled
# build (pgrx's), the randomized load trips a PostgreSQL assertion in heap_update
# (ON CONFLICT DO UPDATE after a rolled-back savepoint; also on 16.x, without any
# extension). Time the overhead only on a release build, as test/bench/write_path.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
db=facts_m0_run

# fresh: a new database with the family installed.
fresh() {
  psql -qX -d postgres -c "SET client_min_messages = warning" -c "DROP DATABASE IF EXISTS $db" -c "CREATE DATABASE $db"
  psql -qX -v ON_ERROR_STOP=1 -d "$db" -f "$here/schema.sql" -f "$here/generate.sql" \
       -f "$here/verify.sql" -f "$here/load.sql" \
       -c "UPDATE facts_m0.family SET declaration = declaration || jsonb_build_object('lock', ${FACTS_M0_LOCK:-true})" \
       -c "SELECT facts_m0.install('sales')" >/dev/null
}

correctness() {
  local rounds="${1:-10}" statements="${2:-100}" r problems rows
  fresh
  for ((r = 1; r <= rounds; r++)); do
    psql -qXAt -v ON_ERROR_STOP=1 -d "$db" -c "SELECT facts_m0.random_load($r * 0.01, $statements)" >/dev/null
    problems="$(psql -qXAt -d "$db" -c "SELECT count(*) FROM facts_m0.verify('sales')")"
    rows="$(psql -qXAt -d "$db" -c "SELECT count(*) FROM tf_sales_day")"
    echo "round $r: $statements statements, $rows day rows, verify: $problems problem(s)"
    [[ "$problems" == 0 ]] || { psql -qX -d "$db" -c "SELECT * FROM facts_m0.verify('sales') LIMIT 10"; exit 1; }
  done
  problems="$(psql -qXAt -d "$db" -c "SELECT count(*) FROM facts_m0.check_zones('sales')")"
  echo "date buckets under Pacific/Kiritimati and America/Adak: $problems problem(s)"
  [[ "$problems" == 0 ]] || { psql -qX -d "$db" -c "SELECT * FROM facts_m0.check_zones('sales')"; exit 1; }
  echo "correctness: PASS"
}

isolation() {
  local regress out rc
  regress="$(pg_config --pkglibdir)/pgxs/src/test/isolation/pg_isolation_regress"
  fresh
  out="$(mktemp -d)"
  rc=0
  "$regress" --use-existing --host="$PGHOST" --port="$PGPORT" --user="$PGUSER" \
    --bindir="$(pg_config --bindir)" --inputdir="$here/isolation" --outputdir="$out" \
    --dbname="$db" --schedule="$here/isolation/isolation_schedule" || rc=$?
  [[ $rc -ne 0 && -f "$out/regression.diffs" ]] && cat "$out/regression.diffs"
  [[ $rc -eq 0 ]] && rm -rf "$out" || echo "results kept in $out"
  return $rc
}

# overhead: per write, ms per transaction, µs of it per inserted row, and WAL bytes
# per inserted row with the
# family's triggers, and the same write with triggers off (session_replication_role
# = replica), each on a fresh copy of a seeded template (100k day rows, 10 tenants).
overhead() {
  local secs="${1:-20}" clients="${2:-1}" tpl=facts_m0_tpl w rows am aw bm bw tmp
  # shellcheck source=../write_path/lib.sh
  source "$here/../write_path/lib.sh"
  if ! psql -d "$tpl" -tAc "SELECT 1 FROM tf_sales_month LIMIT 1" >/dev/null 2>&1; then
    db=$tpl fresh
    psql -qX -d "$tpl" -c "INSERT INTO tf_sales_day (tenant_id, entity_id, category, day, amount, qty)
      SELECT md5((1 + g % 10)::text)::uuid, g % 2000, (ARRAY['a', 'b', 'c'])[1 + g % 3],
             date '2025-01-01' + g % 1095, 1.50, 1 FROM generate_series(1, 100000) g" -c "VACUUM ANALYZE"
  fi
  tmp="$(mktemp -d)"
  printf '%-12s %10s %12s %12s %10s %12s %7s\n' write ms/txn us/row WAL_B/row plain_ms plain_WAL/row ratio
  for w in insert_1 insert_1000; do
    rows="${w#insert_}"
    { grep '^[\]' "$here/writes/$w.sql"; echo "BEGIN;"; echo "SET LOCAL session_replication_role = replica;"
      grep -v '^[\]' "$here/writes/$w.sql"; echo "COMMIT;"; } > "$tmp/$w.plain.sql"
    fresh_copy "$tpl" "$db"; read -r _ am aw < <(measure "$db" "$here/writes/$w.sql" "$secs" "$clients")
    fresh_copy "$tpl" "$db"; read -r _ bm bw < <(measure "$db" "$tmp/$w.plain.sql" "$secs" "$clients")
    printf '%-12s %10s %12s %12s %10s %12s %6sx\n' "$w" "$am" \
      "$(awk -v m="$am" -v r="$rows" 'BEGIN{printf "%.1f", m*1000/r}')" \
      "$(awk -v w="$aw" -v r="$rows" 'BEGIN{printf "%.0f", w/r}')" "$bm" \
      "$(awk -v w="$bw" -v r="$rows" 'BEGIN{printf "%.0f", w/r}')" \
      "$(awk -v a="$am" -v b="$bm" 'BEGIN{printf "%.2f", (b>0)?a/b:0}')"
  done
  rm -rf "$tmp"
}

case "${1:-correctness}" in
  correctness) shift || true; correctness "$@" ;;
  isolation) isolation ;;
  overhead) shift; overhead "$@" ;;
  *) echo "usage: $0 [correctness|isolation|overhead] …" >&2; exit 2 ;;
esac

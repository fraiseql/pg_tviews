#!/usr/bin/env bash
# ADR 0260 M0: one fact family (day → month → year, and a category × month rollup)
# kept by generated statement triggers, on a database without pg_tviews_facts.
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/facts_m0/run.sh [correctness|isolation|overhead] …
#   correctness [rounds] [statements]  randomized load, verify after each round (default 10 × 100)
#   isolation                          concurrent loads (isolation/specs), via pg_isolation_regress
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
       -f "$here/verify.sql" -f "$here/load.sql" -c "SELECT facts_m0.install('sales')" >/dev/null
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

case "${1:-correctness}" in
  correctness) shift || true; correctness "$@" ;;
  *) echo "usage: $0 [correctness|isolation|overhead] …" >&2; exit 2 ;;
esac

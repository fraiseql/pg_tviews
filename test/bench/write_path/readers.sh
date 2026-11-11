#!/usr/bin/env bash
# What one statement costs per TVIEW reading its table (#298): an UPDATE of every
# row of a 5,000-row table read by E TVIEWs, for each E. Each E runs on a fresh copy
# of its seeded database. Prints ms per statement and per TVIEW.
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/write_path/readers.sh [seconds]
#   READERS="10 40 80"   the numbers of TVIEWs reading the table (default)
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$here/lib.sh"
secs="${1:-20}"
read -r -a readers <<<"${READERS:-10 40 80}"
db=pg_tviews_readers_bench_run
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"; psql -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT
printf 'UPDATE r.tb_src SET v = v + 1;\n' > "$tmp/w.sql"
printf '%7s %12s %12s\n' readers ms/stmt ms/tview
for e in "${readers[@]}"; do
  tpl="pg_tviews_readers_bench_$e"
  if ! psql -X -d "$tpl" -tAc "SELECT 1 FROM r.tv_r$((e - 1)) LIMIT 1" >/dev/null 2>&1; then
    psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $tpl" -c "CREATE DATABASE $tpl"
    psql -X -q -v ON_ERROR_STOP=1 -d "$tpl" >/dev/null <<SQL
SET client_min_messages = warning;
SET pg_tviews.max_queue_size = 2000000;
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
CREATE SCHEMA r;
CREATE TABLE r.tb_src (pk_src int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v int NOT NULL);
INSERT INTO r.tb_src (pk_src, v) SELECT x, 0 FROM generate_series(1, 5000) x;
DO \$\$
BEGIN
  FOR i IN 0 .. $e - 1 LOOP
    PERFORM tviews.pg_tviews_create(format('r.tv_r%s', i),
      format('SELECT pk_src AS pk_r%s, id, jsonb_build_object(''v'', v, ''r'', %s) AS data FROM r.tb_src', i, i));
  END LOOP;
END \$\$;
VACUUM ANALYZE;
SQL
  fi
  fresh_copy "$tpl" "$db"
  read -r _ ms _ < <(PGOPTIONS="-c pg_tviews.max_queue_size=2000000" measure "$db" "$tmp/w.sql" "$secs" 1 "-c pg_tviews.max_queue_size=2000000")
  printf '%7s %12s %12s\n' "$e" "$ms" "$(awk -v m="$ms" -v e="$e" 'BEGIN { printf "%.2f", m / e }')"
done

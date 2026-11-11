#!/usr/bin/env bash
# What the number of TVIEWs in a database costs a write that touches one of them
# (#299): N independent single-table TVIEWs, one-row updates of one of them. Each
# size runs on a fresh copy of its seeded database. Prints ms per write.
#
# Usage: PGHOST=… PGPORT=… PGUSER=… ./test/bench/write_path/graph_size.sh [seconds] [clients]
#   SIZES="3 303 1003"   the numbers of TVIEWs (default)
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$here/lib.sh"
secs="${1:-10}"; clients="${2:-1}"
read -r -a sizes <<<"${SIZES:-3 303 1003}"
db=pg_tviews_graph_bench_run
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"; psql -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT
printf 'UPDATE g.tb_t0 SET v = v + 1 WHERE pk_t0 = 1 + (random() * 999)::int;\n' > "$tmp/w.sql"
printf '%7s %10s %10s\n' tviews tps ms/write
for n in "${sizes[@]}"; do
  tpl="pg_tviews_graph_bench_$n"
  if ! psql -X -d "$tpl" -tAc "SELECT 1 FROM g.tv_t$((n - 1)) LIMIT 1" >/dev/null 2>&1; then
    psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $tpl" -c "CREATE DATABASE $tpl"
    psql -X -q -v ON_ERROR_STOP=1 -d "$tpl" >/dev/null <<SQL
SET client_min_messages = warning;
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
CREATE SCHEMA g;
DO \$\$
BEGIN
  FOR i IN 0 .. $n - 1 LOOP
    EXECUTE format('CREATE TABLE g.tb_t%s (pk_t%s int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), v int NOT NULL)', i, i);
    EXECUTE format('INSERT INTO g.tb_t%s (pk_t%s, v) SELECT x, 0 FROM generate_series(1, %s) x', i, i,
                   CASE WHEN i = 0 THEN 1000 ELSE 1 END);
    PERFORM tviews.pg_tviews_create(format('g.tv_t%s', i),
      format('SELECT pk_t%s, id, jsonb_build_object(''v'', v) AS data FROM g.tb_t%s', i, i));
  END LOOP;
END \$\$;
VACUUM ANALYZE;
SQL
  fi
  fresh_copy "$tpl" "$db"
  read -r tps ms _ < <(measure "$db" "$tmp/w.sql" "$secs" "$clients")
  printf '%7s %10s %10s\n' "$n" "$tps" "$ms"
done

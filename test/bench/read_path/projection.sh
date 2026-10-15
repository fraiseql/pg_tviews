#!/usr/bin/env bash
# A page of 20 posts selecting id and title only (no comments), projected from data as
# fraiseql projects fields: what the comments array costs the reads that don't ask for it.
# Usage: PGHOST=… PGPORT=… PGUSER=… [POSTS=2000 SUFFIX=_50] ./projection.sh [rounds]
set -euo pipefail
# shellcheck source=../write_path/lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../write_path/lib.sh"
rounds="${1:-3}"; P="${POSTS:-20000}"; S="${SUFFIX:-}"; db=rc_run
trap 'psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT
for round in $(seq 1 "$rounds"); do
  for design in embedded composed; do
    fresh_copy "rc_$design$S" "$db"
    for pass in warm measured; do
      psql -X -q -d "$db" 2>&1 <<SQL | sed -n "s/.*NOTICE:  measured /$round $design /p"
DO \$\$ DECLARE r record; n bigint := 0; t0 timestamptz := clock_timestamp(); BEGIN
  FOR i IN 1..4000 LOOP
    FOR r IN SELECT jsonb_build_object('id', p.data->'id', 'title', p.data->'title')::text AS t
             FROM bench.tv_post p WHERE p.pk_post > (i * 397) % ($P - 20) ORDER BY p.pk_post LIMIT 20
    LOOP n := n + length(r.t); END LOOP;
  END LOOP;
  RAISE NOTICE '$pass % us/page', round(extract(epoch FROM clock_timestamp() - t0) * 1e6 / 4000, 1);
END \$\$;
SQL
    done
  done
done

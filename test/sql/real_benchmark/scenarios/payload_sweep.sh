#!/usr/bin/env bash
# Physical scenario: payload size sweep.
#
# One entity whose `data` carries an incompressible payload of SIZE bytes next to
# a small `counter`. Each measured step changes only `counter`, so any TOAST and
# WAL growth with SIZE is the cost of rewriting the whole jsonb document on a
# one-field change. Sizes straddle the ~2 KB TOAST threshold.
#
# Steps per size: update_single (UPDATES single-row autocommit statements) and
# update_batch (one statement touching 10% of rows).
#
# Usage:
#   ./payload_sweep.sh [--dry-run]
# Env: SIZES ("100 1024 4096 16384 65536"), ROWS (2000), UPDATES (200), MODES, RUN_DIR

set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=SCRIPTDIR/../lib/common.sh
source "$here/../lib/common.sh"

SIZES="${SIZES:-100 1024 4096 16384 65536}"
ROWS="${ROWS:-2000}"
UPDATES="${UPDATES:-200}"
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && { DRY_RUN=1; SIZES="100 4096"; ROWS=50; MODES="unlogged"; }

setup_sql() {  # $1 = size, $2 = mode
  cat <<SQL
SET client_min_messages TO WARNING;
CREATE TABLE tb_doc (
    pk_doc  int PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    counter int NOT NULL DEFAULT 0,
    payload text NOT NULL
);
-- md5 hex is incompressible enough that pglz cannot shrink it below the TOAST
-- threshold, so SIZE is close to the stored size.
INSERT INTO tb_doc (pk_doc, payload)
SELECT g, left((SELECT string_agg(md5(g::text || ':' || s), '')
                FROM generate_series(1, ceil($1 / 32.0)::int) s), $1)
FROM generate_series(1, $ROWS) g;
$(mode_sql "$2")
SELECT pg_tviews_create('tv_doc', \$v\$
    SELECT pk_doc, id, jsonb_build_object('counter', counter, 'payload', payload) AS data
    FROM tb_doc \$v\$);
SQL
}

dry_run_check() {  # $1 = db, $2 = size
  $PSQL -d "$1" -tA -c "
    SELECT CASE WHEN (SELECT count(*) FROM tv_doc) = $ROWS
                 AND (SELECT min(length(data->>'payload')) FROM tv_doc) = $2
                THEN 'DRY_OK' ELSE 'DRY_FAIL' END"
}

workload_sql() {  # $1 = size
  local step=$(( ROWS / UPDATES )); [[ $step -lt 1 ]] && step=1
  phys_step_begin
  for ((i = 0; i < UPDATES; i++)); do
    echo "UPDATE tb_doc SET counter = counter + 1 WHERE pk_doc = $(( 1 + (i * step) % ROWS ));"
  done
  phys_snap "update_single" "$UPDATES"
  echo "UPDATE tb_doc SET counter = counter + 1 WHERE pk_doc % 10 = 0;"
  phys_snap "update_batch" 1
  divergence_gate tv_doc v_doc pk_doc
}

record_env
for mode in $MODES; do
  for size in $SIZES; do
    scenario="payload_${size}"
    db="bench_phys_payload"
    echo "== $scenario ($mode)"
    db_fresh "$db"
    bench_install "$db"
    tmp="$(mktemp)"
    setup_sql "$size" "$mode" >"$tmp"
    run_script "$db" "$tmp" "$RUN_DIR/${scenario}_${mode}.setup.log" || exit 1
    if [[ $DRY_RUN == 1 ]]; then
      res="$(dry_run_check "$db" "$size")"; echo "  $res"
      [[ "$res" == DRY_OK ]] || exit 1
      continue
    fi
    workload_sql "$size" >"$tmp"
    run_script "$db" "$tmp" "$RUN_DIR/${scenario}_${mode}.log" || exit 1
    phys_dump "$db" "$scenario" "$mode" || exit 1
    echo "UPDATE tb_doc SET counter = counter + 1 WHERE pk_doc = 1;" >"$tmp"
    explain_pass "$db" "$tmp" "${scenario}_${mode}" 'tv_doc'
    rm -f "$tmp"
  done
done
db_exec "DROP DATABASE IF EXISTS bench_phys_payload"
echo "payload_sweep done -> $RUN_DIR"

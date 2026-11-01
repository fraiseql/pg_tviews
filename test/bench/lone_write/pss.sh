#!/usr/bin/env bash
# The statements one lone write runs, from pg_stat_statements: per write kind, the
# SPI statements pg_tviews ran, how many were planned, and their times. Each write
# is `BEGIN; <write>; SELECT tviews.pg_tviews_flush_and_report(...); ROLLBACK`, run
# 10 times to warm the backend's kept plans, then WRITES times measured.
#
# Usage: pss.sh <label> [package root]
#   With a package root (from `cargo pgrx package --out-dir`), that build is
#   installed first (swap.sh). Without one, the installed build is measured.
#
# Environment: SCHEMA (setup | wide, default setup), KINDS (default: every kind of
# the schema), WRITES (50), OUT (output directory, default ./out), PG_BIN (the
# server's bin directory), PGHOST/PGPORT/PGUSER.
#
# Needs pg_stat_statements loaded with track = all and track_planning = on (README).
#
# Counts are exact. Times move with the host's load: compare builds by `per_compute`,
# every time divided by the compute statement's (the recompute upsert, `WITH src AS
# (...)`), which does the same work in every build. Kinds that recompute no row
# (fan-out patches) have no compute statement: compare their counts.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PGHOST="${PGHOST:-localhost}" PGPORT="${PGPORT:-28818}" PGUSER="${PGUSER:-postgres}"
PG_BIN="${PG_BIN:-$HOME/.pgrx/18.1/pgrx-install/bin}"
SCHEMA="${SCHEMA:-setup}"
WRITES="${WRITES:-50}"
OUT="${OUT:-$here/out}"
label="${1:?usage: pss.sh <label> [package root]}"
mkdir -p "$OUT"
[[ -n "${2:-}" ]] && "$here/swap.sh" "$2" >/dev/null

declare -A W=(
  [group_rename]="UPDATE tb_group SET name = name || 'x' WHERE pk_group = 1"
  [soft_delete]="UPDATE tb_item SET deleted_at = now() WHERE pk_item = 9"
  [move]="UPDATE tb_item SET start_date = DATE '2026-12-01' WHERE pk_item = 10"
  [item_rename]="UPDATE tb_item SET name = name || 'x' WHERE pk_item = 11"
  [owner_rename]="UPDATE tb_owner SET name = name || 'x' WHERE pk_owner = 1"
  [agreement_rename]="UPDATE tb_agreement SET name = name || 'x' WHERE pk_agreement = 1"
  [asset_edit]="UPDATE tb_asset SET serial = serial || 'x' WHERE pk_asset = 1"
)
case "$SCHEMA" in
  setup) default_kinds="group_rename soft_delete move item_rename owner_rename" ;;
  wide) default_kinds="soft_delete move item_rename group_rename agreement_rename asset_edit" ;;
  *) echo "SCHEMA is setup or wide" >&2; exit 2 ;;
esac

psql() { "$PG_BIN/psql" -X -v ON_ERROR_STOP=1 "$@"; }
"$PG_BIN/dropdb" --if-exists lone_write 2>/dev/null
"$PG_BIN/createdb" lone_write
psql -q -d lone_write -f "$here/$SCHEMA.sql" >/dev/null
psql -q -d lone_write -c "CREATE EXTENSION IF NOT EXISTS pg_stat_statements"

for kind in ${KINDS:-$default_kinds}; do
  write="${W[$kind]}"
  script="$(mktemp)"
  {
    echo "\\set QUIET on"
    echo "SET client_min_messages = warning;"
    for _ in $(seq 10); do
      echo "BEGIN; $write; SELECT tviews.pg_tviews_flush_and_report(include_data => false) \\g /dev/null"
      echo "ROLLBACK;"
    done
    echo "SELECT pg_stat_statements_reset() \\g /dev/null"
    for _ in $(seq "$WRITES"); do
      echo "BEGIN; $write; SELECT tviews.pg_tviews_flush_and_report(include_data => false) \\g /dev/null"
      echo "ROLLBACK;"
    done
  } >"$script"
  psql -q -d lone_write -f "$script"
  rm -f "$script"
  # One summary line: calls, plans, plan and execution ms of the nested statements,
  # the compute's ms, and the nested total per compute.
  psql -d lone_write -At -F' ' -c "
    WITH s AS (SELECT * FROM pg_stat_statements WHERE NOT toplevel),
    t AS (SELECT sum(calls) / $WRITES.0 AS calls, sum(plans) / $WRITES.0 AS plans,
                 sum(total_plan_time) / $WRITES AS plan_ms,
                 sum(total_exec_time) / $WRITES AS exec_ms,
                 sum(total_exec_time) FILTER (WHERE query LIKE 'WITH src AS (%') / $WRITES AS compute_ms
          FROM s)
    SELECT '$label', '$SCHEMA', '$kind', round(calls, 1), round(plans, 1),
           round(plan_ms::numeric, 3), round(exec_ms::numeric, 3),
           round(compute_ms::numeric, 3),
           round(((plan_ms + exec_ms) / NULLIF(compute_ms, 0))::numeric, 2)
    FROM t" | tee -a "$OUT/summary.txt"
  # Every nested statement: calls, plans and times per write.
  psql -d lone_write -At -F' | ' -c "
    SELECT round(calls / $WRITES.0, 1), round(plans / $WRITES.0, 1),
           round((total_plan_time / $WRITES)::numeric, 3),
           round((total_exec_time / $WRITES)::numeric, 3),
           left(regexp_replace(query, '\s+', ' ', 'g'), 160)
    FROM pg_stat_statements WHERE NOT toplevel AND calls >= $WRITES / 10 ORDER BY total_exec_time DESC" \
    >"$OUT/$label-$SCHEMA-$kind.txt"
done

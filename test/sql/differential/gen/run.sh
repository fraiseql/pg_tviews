#!/usr/bin/env bash
# Generated shapes: per seed, gen.py writes random schemas in the project's
# convention and TVIEW definitions drawn from its grammar (joins, embedded TVIEWs,
# subquery aggregates, jsonb_agg arrays, UNION ALL, CTEs, GROUP BY, DISTINCT ON,
# soft-delete filters), then random writes: single- and multi-row, rows moving
# between parents, key changes, transactions, and mutation functions writing in a
# few statements and reading a TVIEW back. Every TVIEW must equal its view after
# each top-level statement, and inside each mutation after its writes.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/sql/differential/gen/run.sh [SHAPES] [SEEDS] [STATEMENTS]
#
# SHAPES TVIEW definitions per seed (default 20), split into cases of 3 to 6, each
# case its own database; SEEDS a space-separated list (default "1 2 3"); STATEMENTS
# random writes per case (default 40). A definition is created, or refused with a
# documented SQLSTATE (REFUSED, listed in lib.sql); any other error, a divergence or
# a stale read fails the case, which prints its seed and keeps its SQL.
#
# MODES (test/lib/modes.sh) runs every case once per mode, as run.sh does.
# GEN_FEATURES (comma-separated) adds the opt-in writes of gen.py's OPT_IN.
# GEN_SELFTEST=1 writes with session_replication_role = replica, so no pg_tviews
# trigger fires: every case must then fail, which proves the oracle sees a refresh
# that does not happen.

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../../../lib/modes.sh
source "$here/../../../lib/modes.sh"
check_modes
shapes="${1:-20}"
seeds="${2:-1 2 3}"
statements="${3:-40}"
selftest="${GEN_SELFTEST:-0}"
# Opt-in writes of gen.py (OPT_IN), each left out while its defect is open.
features="${GEN_FEATURES-}"
db="pg_tviews_gen_$$"
work="$(mktemp -d)"
# Failing cases are copied to GEN_KEEP (a new temporary directory by default).
keep="${GEN_KEEP-}"
trap 'rm -rf "$work"; psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT

failed=0
cases=0
for seed in $seeds; do
  mkdir -p "$work/$seed"
  python3 -I "$here/gen.py" "$seed" "$shapes" "$statements" "$work/$seed" "$features" >"$work/$seed/index" \
    || { echo "ERROR: gen.py failed for seed $seed"; exit 2; }
  while read -r file n; do
    for mode in $MODES; do
      PGOPTIONS="$(mode_options "$mode")"
      [[ "$selftest" == 1 ]] && PGOPTIONS+=" -c session_replication_role=replica"
      export PGOPTIONS
      cases=$((cases+1))
      psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1
      psql -d postgres -qc "CREATE DATABASE $db" >/dev/null || { echo "ERROR: cannot create $db"; exit 2; }
      psql -d postgres -qc "ALTER DATABASE $db SET search_path = \"\$user\", public, tviews" >/dev/null
      out="$work/out"
      psql -d "$db" -X -q -v ON_ERROR_STOP=1 -f "$here/lib.sql" -f "$work/$seed/$file" >"$out" 2>&1
      rc=$?
      refused="$(grep -c 'REFUSED' "$out")"
      name="seed $seed ${file%.sql} [$mode] ($n shapes, $statements statements, $refused refused)"
      if [[ "$selftest" == 1 ]]; then
        if [[ $rc -ne 0 ]] && grep -qE 'DIVERGED|STALE' "$out"; then
          echo "PASS  $name: the oracle saw the missing refresh"
        else
          echo "FAIL  $name: no divergence without triggers"
          failed=$((failed+1))
        fi
      elif [[ $rc -eq 0 ]]; then
        echo "PASS  $name"
      else
        failed=$((failed+1))
        [[ -n "$keep" ]] || keep="$(mktemp -d -t pg_tviews_gen_failed.XXXXXX)"
        mkdir -p "$keep"
        cp "$work/$seed/$file" "$keep/seed$seed-$file"
        echo "FAIL  $name"
        grep -E -A6 'ERROR' "$out" | head -20 | sed 's/^/      /'
        echo "      replay: psql -f $here/lib.sql -f $keep/seed$seed-$file"
        echo "      regenerate: python3 -I $here/gen.py $seed $shapes $statements <dir> $features"
      fi
      grep -E 'REFUSED|SKIPPED' "$out" | sed 's/^.*WARNING: */      /'
    done
  done <"$work/$seed/index"
done
unset PGOPTIONS
echo "$((cases-failed))/$cases cases passed"
[[ "$failed" -eq 0 ]]

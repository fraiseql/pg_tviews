#!/usr/bin/env bash
# Differential suite: every TVIEW shape of schema.sql must equal its backing view
# after each of N seeded random statements (single- and multi-row writes, key
# changes, rows moving between parents).
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/sql/differential/run.sh [N] [SEEDS]
#
# N statements per seed (default 60), SEEDS a space-separated list (default "1 2 3").
# A divergence prints the statement and the rows that differ, and fails the run.

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
n="${1:-60}"
seeds="${2:-1 2 3}"
db="pg_tviews_differential_$$"
# Shapes with an open defect (comma-separated tv_* names): their divergence is
# reported, not failed.
xfail="${HARNESS_XFAIL-}"

failed=0
for seed in $seeds; do
  psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1
  psql -d postgres -qc "CREATE DATABASE $db" >/dev/null || { echo "ERROR: cannot create $db"; exit 2; }
  psql -d postgres -qc "ALTER DATABASE $db SET search_path = \"\$user\", public, tviews" >/dev/null
  out="$(mktemp)"
  if psql -d "$db" -X -q -v ON_ERROR_STOP=1 -v xfail="$xfail" -f "$here/schema.sql" >"$out" 2>&1 \
     && psql -d "$db" -X -q -v ON_ERROR_STOP=1 >>"$out" 2>&1 <<SQL
SET client_min_messages TO WARNING;
SET harness.xfail = '$xfail';
SELECT setseed(1.0 / ($seed + 1)) \\g /dev/null
SELECT s FROM harness_script($n) s \\gexec
SQL
  then
    echo "PASS  seed $seed ($n statements)"
  else
    echo "FAIL  seed $seed: $(grep -E 'ERROR' "$out" | head -1)"
    failed=$((failed+1))
  fi
  grep -E 'XFAIL' "$out" | sed 's/^.*WARNING: */      /'
  rm -f "$out"
done
psql -d postgres -qc "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1
[[ "$failed" -eq 0 ]]

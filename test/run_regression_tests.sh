#!/usr/bin/env bash
# Run the pg_tviews open-issue regression suite (test/sql/regress_issue_*.sql).
#
# Each test runs in a throwaway database. Tests that require the real jsonb_delta
# extension are skipped (not failed) when it is not installed in the cluster, so
# this script is safe to run in CI images that ship only pg_tviews.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/run_regression_tests.sh
#
# Honors PGHOST/PGPORT/PGUSER (defaults: localhost / 28818 / postgres).

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sqldir="$here/sql"
tmpdb="pg_tviews_regress_$$"

psql -d postgres -tAc "SELECT 1" >/dev/null 2>&1 || {
  echo "ERROR: cannot connect to PostgreSQL at $PGHOST:$PGPORT as $PGUSER"; exit 2; }

# Is the real jsonb_delta extension available in this cluster?
have_jsonb_delta=$(psql -d postgres -tAc \
  "SELECT count(*) FROM pg_available_extensions WHERE name='jsonb_delta'" 2>/dev/null || echo 0)

pass=0 fail=0 skip=0 failed_names=""
for f in "$sqldir"/regress_issue_*.sql; do
  name="$(basename "$f")"
  # The fallback test deliberately runs without jsonb_delta; everything else needs it.
  if [[ "$name" != *fallback* && "$have_jsonb_delta" == "0" ]]; then
    echo "SKIP  $name (jsonb_delta not installed)"; skip=$((skip+1)); continue
  fi
  psql -d postgres -c "DROP DATABASE IF EXISTS $tmpdb" >/dev/null 2>&1
  psql -d postgres -c "CREATE DATABASE $tmpdb" >/dev/null 2>&1
  if psql -d "$tmpdb" -q -v ON_ERROR_STOP=1 -f "$f" >/tmp/$tmpdb.out 2>&1; then
    echo "PASS  $name"; pass=$((pass+1))
  else
    echo "FAIL  $name -> $(grep -iE 'ERROR|EXCEPTION' /tmp/$tmpdb.out | head -1)"
    fail=$((fail+1)); failed_names="$failed_names $name"
  fi
done
psql -d postgres -c "DROP DATABASE IF EXISTS $tmpdb" >/dev/null 2>&1

echo "----------------------------------------"
echo "regression: $pass passed, $fail failed, $skip skipped"
[[ -n "$failed_names" ]] && echo "failed:$failed_names"
[[ "$fail" -eq 0 ]]

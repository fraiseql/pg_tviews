#!/usr/bin/env bash
# Run the pg_tviews numbered integration suite (test/sql/[0-9]*.sql).
#
# Each file is run standalone in a throwaway database (it sets up its own
# extensions). Files under test/sql/quarantine/ are intentionally excluded
# (see that directory's README). These integration tests require the real
# jsonb_delta extension; if it is not installed the whole suite is skipped
# (not failed) so this script is safe to run before jsonb_delta is wired in.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/run_integration_tests.sh
#
# Honors PGHOST/PGPORT/PGUSER (defaults: localhost / 28818 / postgres).

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sqldir="$here/sql"
tmpdb="pg_tviews_integ_$$"

psql -d postgres -tAc "SELECT 1" >/dev/null 2>&1 || {
  echo "ERROR: cannot connect to PostgreSQL at $PGHOST:$PGPORT as $PGUSER"; exit 2; }

have_jsonb_delta=$(psql -d postgres -tAc \
  "SELECT count(*) FROM pg_available_extensions WHERE name='jsonb_delta'" 2>/dev/null || echo 0)
if [[ "$have_jsonb_delta" == "0" ]]; then
  echo "SKIP-ALL: jsonb_delta not installed; the numbered integration suite requires it."
  exit 0
fi

# Only files directly in sql/ (the quarantine subdir is excluded by the glob).
shopt -s nullglob
files=("$sqldir"/[0-9]*.sql)
shopt -u nullglob

pass=0 fail=0 failed_names=""
for f in "${files[@]}"; do
  name="$(basename "$f")"
  psql -d postgres -c "DROP DATABASE IF EXISTS $tmpdb" >/dev/null 2>&1
  psql -d postgres -c "CREATE DATABASE $tmpdb" >/dev/null 2>&1
  if psql -d "$tmpdb" -q -v ON_ERROR_STOP=1 -f "$f" >/tmp/$tmpdb.out 2>&1; then
    echo "PASS  $name"; pass=$((pass+1))
  else
    echo "FAIL  $name -> $(grep -E 'psql:.*(ERROR|FATAL):' /tmp/$tmpdb.out | head -1)"
    fail=$((fail+1)); failed_names="$failed_names $name"
  fi
done
psql -d postgres -c "DROP DATABASE IF EXISTS $tmpdb" >/dev/null 2>&1

echo "----------------------------------------"
echo "integration: $pass passed, $fail failed (of ${#files[@]} files)"
[[ -n "$failed_names" ]] && echo "failed:$failed_names"
[[ "$fail" -eq 0 && "$pass" -gt 0 ]]

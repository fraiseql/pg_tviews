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
  # The extension lives in schema tviews; tests call its functions unqualified.
  psql -d postgres -qc "ALTER DATABASE $tmpdb SET search_path = \"\$user\", public, tviews" >/dev/null \
    || { echo "ERROR: could not create test database $tmpdb"; exit 2; }
  if psql -d "$tmpdb" -q -v ON_ERROR_STOP=1 -f "$f" >/tmp/$tmpdb.out 2>&1; then
    if grep -q '^-- expect-quiet' "$f" && grep -qE 'EVENT TRIGGER|DEBUG:|spi_run_ddl' /tmp/$tmpdb.out; then
      echo "FAIL  $name -> unexpected diagnostics: $(grep -E 'EVENT TRIGGER|DEBUG:|spi_run_ddl' /tmp/$tmpdb.out | head -1)"
      fail=$((fail+1)); failed_names="$failed_names $name"; continue
    fi
    # Every `-- expect-output: <text>` line must appear in the output, every
    # `-- expect-once: <text>` line exactly once, and no `-- reject-output: <text>` line.
    missing="" unwanted=""
    while IFS= read -r once; do
      [[ -n "$once" ]] && [[ "$(grep -cF -- "$once" /tmp/$tmpdb.out)" != 1 ]] && { missing="$once (exactly once)"; break; }
    done < <(sed -n 's/^-- expect-once: //p' "$f")
    while IFS= read -r want; do
      [[ -n "$want" ]] && ! grep -qF -- "$want" /tmp/$tmpdb.out && { missing="$want"; break; }
    done < <(sed -n 's/^-- expect-output: //p' "$f")
    while IFS= read -r reject; do
      [[ -n "$reject" ]] && grep -qF -- "$reject" /tmp/$tmpdb.out && { unwanted="$reject"; break; }
    done < <(sed -n 's/^-- reject-output: //p' "$f")
    if [[ -n "$missing" ]]; then
      echo "FAIL  $name -> expected output containing '$missing'"
      fail=$((fail+1)); failed_names="$failed_names $name"; continue
    fi
    if [[ -n "$unwanted" ]]; then
      echo "FAIL  $name -> unexpected output containing '$unwanted'"
      fail=$((fail+1)); failed_names="$failed_names $name"; continue
    fi
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

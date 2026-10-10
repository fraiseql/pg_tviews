#!/usr/bin/env bash
# Run the pg_tviews regression suite (test/sql/regress_*.sql).
#
# Each test runs in a throwaway database. Tests that require the real jsonb_delta
# extension are skipped (not failed) when it is not installed in the cluster, so
# this script is safe to run in CI images that ship only pg_tviews.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/run_regression_tests.sh [file ...]
#
# With file names, only those tests run. A test fails when psql exits non-zero,
# whatever it printed. A test that cannot run here echoes `SKIP: <reason>` and
# quits; it is counted as skipped.
#
# Honors PGHOST/PGPORT/PGUSER (defaults: localhost / 28818 / postgres).
#
# A test reproducing an open bug carries `-- known-failing: <issue>`: its failure
# is reported as XFAIL and does not fail the suite; once it passes it FAILs until
# the marker is removed.

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

pass=0 fail=0 skip=0 xfail=0 failed_names=""

# Record the outcome of test $1: "" when it passed, else why it failed.
verdict() {
  local name="$1" why="$2" known="$3"
  if [[ -n "$known" && -n "$why" ]]; then
    echo "XFAIL $name ($known) -> $why"; xfail=$((xfail+1))
  elif [[ -n "$known" ]]; then
    echo "FAIL  $name -> passes, but is marked known-failing ($known): remove the marker"
    fail=$((fail+1)); failed_names="$failed_names $name"
  elif [[ -n "$why" ]]; then
    echo "FAIL  $name -> $why"; fail=$((fail+1)); failed_names="$failed_names $name"
  else
    echo "PASS  $name"; pass=$((pass+1))
  fi
}

# Judge test file $1 from psql's exit status $2 and its output in $3.
report_result() {
  local f="$1" rc="$2" out="$3" name known why="" once want reject
  name="$(basename "$f")"
  known=$(sed -n 's/^-- known-failing: //p' "$f" | head -1)
  if [[ "$rc" -ne 0 ]]; then
    why="$(grep -E '(ERROR|FATAL|PANIC):' "$out" | head -1)"
    [[ -n "$why" ]] || why="$(grep -iE 'error|exception' "$out" | head -1)"
    verdict "$name" "${why:-psql exit $rc}" "$known"
    return
  fi
  # A file that skips itself echoes `SKIP: <reason>` and quits early.
  if grep -q '^SKIP: ' "$out"; then
    echo "SKIP  $name ($(sed -n 's/^SKIP: //p' "$out" | head -1))"; skip=$((skip+1))
    return
  fi
  if grep -q '^-- expect-quiet' "$f" && grep -qE 'EVENT TRIGGER|DEBUG:|spi_run_ddl' "$out"; then
    verdict "$name" "unexpected diagnostics: $(grep -E 'EVENT TRIGGER|DEBUG:|spi_run_ddl' "$out" | head -1)" "$known"
    return
  fi
  # Every `-- expect-output: <text>` line must appear in the output, every
  # `-- expect-once: <text>` line exactly once, and no `-- reject-output: <text>` line.
  while IFS= read -r once; do
    [[ -n "$once" ]] && [[ "$(grep -cF -- "$once" "$out")" != 1 ]] && { why="expected output containing '$once (exactly once)'"; break; }
  done < <(sed -n 's/^-- expect-once: //p' "$f")
  if [[ -z "$why" ]]; then
    while IFS= read -r want; do
      [[ -n "$want" ]] && ! grep -qF -- "$want" "$out" && { why="expected output containing '$want'"; break; }
    done < <(sed -n 's/^-- expect-output: //p' "$f")
  fi
  # Refresh work still queued at COMMIT is a missing flush: rejected unless the
  # file expects it.
  if [[ -z "$why" ]] && ! grep -qF -- "-- expect-once: queued refreshes" "$f" \
     && grep -qF "queued refreshes for" "$out"; then
    why="unexpected output containing 'queued refreshes for'"
  fi
  if [[ -z "$why" ]]; then
    while IFS= read -r reject; do
      [[ -n "$reject" ]] && grep -qF -- "$reject" "$out" && { why="unexpected output containing '$reject'"; break; }
    done < <(sed -n 's/^-- reject-output: //p' "$f")
  fi
  verdict "$name" "$why" "$known"
}

# Run every regress file, or only the ones named on the command line.
if [[ $# -gt 0 ]]; then
  files=()
  for arg in "$@"; do files+=("$sqldir/$(basename "$arg")"); done
else
  files=("$sqldir"/regress_*.sql)
fi
out="$(mktemp)"
for f in "${files[@]}"; do
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
  psql -d "$tmpdb" -q -v ON_ERROR_STOP=1 -f "$f" >"$out" 2>&1
  report_result "$f" "$?" "$out"
done
rm -f "$out"
psql -d postgres -c "DROP DATABASE IF EXISTS $tmpdb" >/dev/null 2>&1

echo "----------------------------------------"
echo "regression: $pass passed, $fail failed, $skip skipped, $xfail known-failing"
[[ -n "$failed_names" ]] && echo "failed:$failed_names"
[[ "$fail" -eq 0 ]]

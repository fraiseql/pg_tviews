#!/usr/bin/env bash
# Harness self-checks: run each lib/*_selftest.{sql,sh} in a throwaway database.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./selftest.sh
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PGHOST="${PGHOST:-localhost}" PGPORT="${PGPORT:-28818}" PGUSER="${PGUSER:-postgres}"
db="bench_selftest_$$"
trap 'psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1' EXIT

rc=0
for f in "$here"/lib/*_selftest.sql; do
  psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1
  psql -X -q -d postgres -c "CREATE DATABASE $db" >/dev/null
  if out="$(psql -X -q -d "$db" -f "$f" 2>&1)" && grep -q '_SELFTEST_OK' <<<"$out"; then
    echo "PASS  $(basename "$f")"
  else
    echo "FAIL  $(basename "$f")"; tail -3 <<<"$out"; rc=1
  fi
done
for f in "$here"/lib/*_selftest.sh; do
  psql -X -q -d postgres -c "DROP DATABASE IF EXISTS $db" >/dev/null 2>&1
  psql -X -q -d postgres -c "CREATE DATABASE $db" >/dev/null
  if out="$("$f" "$db" 2>&1)" && grep -q '_SELFTEST_OK' <<<"$out"; then
    echo "PASS  $(basename "$f")"
  else
    echo "FAIL  $(basename "$f")"; tail -3 <<<"$out"; rc=1
  fi
done
exit $rc

#!/usr/bin/env bash
# Skip mutants: every skip decision of pg_tviews, switched to decide wrongly one at
# a time, must be seen by the generator. Each `S<nn>.mutant` file says what its
# decision skips, how the mutant breaks it, and the generator features
# (GEN_FEATURES) it runs with. The run fails when a mutant survives: no case
# diverges, or a case fails otherwise than by a divergence.
#
# Needs a build with the test-mutants feature (pg_tviews.test_mutant):
#   cargo pgrx install --features pg18,test-hooks,test-mutants ...
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/sql/differential/mutants/run.sh [SHAPES] [SEEDS] [MUTANT...]
#
# SHAPES and SEEDS as for gen/run.sh (default 20 and "1 2 3", the nightly set);
# MUTANT names the mutants to run (default: every file here). MODES as gen/run.sh.

set -u
PGHOST="${PGHOST:-localhost}"
PGPORT="${PGPORT:-28818}"
PGUSER="${PGUSER:-postgres}"
export PGHOST PGPORT PGUSER

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
shapes="${1:-20}"
seeds="${2:-1 2 3}"
shift $(( $# < 2 ? $# : 2 ))
mutants=("$@")
if [[ ${#mutants[@]} -eq 0 ]]; then
  for f in "$here"/S*.mutant; do
    m="$(basename "$f" .mutant)"
    mutants+=("$m")
  done
fi

if [[ "$(psql -d postgres -XAtc "SELECT current_setting('pg_tviews.test_mutant', true) IS NOT NULL")" != t ]]; then
  echo "ERROR: pg_tviews is not built with the test-mutants feature (no pg_tviews.test_mutant)"
  exit 2
fi

survived=0
for m in "${mutants[@]}"; do
  file="$here/$m.mutant"
  [[ -f "$file" ]] || { echo "ERROR: no mutant $m"; exit 2; }
  features="$(sed -n 's/^features:[[:space:]]*//p' "$file")"
  log="$(mktemp)"
  GEN_FEATURES="$features" GEN_PGOPTIONS="-c pg_tviews.test_mutant=$m" \
    "$here/../gen/run.sh" "$shapes" "$seeds" >"$log" 2>&1
  diverged="$(grep -c '^FAIL' "$log")"
  other="$(grep -A6 '^FAIL' "$log" | grep 'ERROR' | grep -cvE 'DIVERGED|STALE|LEAKED')"
  total="$(tail -1 "$log")"
  if [[ "$diverged" -gt 0 && "$other" -eq 0 ]]; then
    echo "KILLED   $m: $diverged case(s) diverged (${total})"
  elif [[ "$diverged" -gt 0 ]]; then
    survived=$((survived+1))
    echo "ERROR    $m: $other case(s) failed otherwise than by a divergence"
    grep -A6 '^FAIL' "$log" | grep 'ERROR' | grep -vE 'DIVERGED|STALE|LEAKED' | head -3 | sed 's/^/      /'
  else
    survived=$((survived+1))
    echo "SURVIVED $m: no case diverged (${total})"
  fi
  rm -f "$log"
done
echo "$(( ${#mutants[@]} - survived ))/${#mutants[@]} mutants killed"
[[ "$survived" -eq 0 ]]

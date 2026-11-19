#!/usr/bin/env bash
# The two independent checks of the read sets, each shown to catch a wrong one.
# On the generator's kinds reading tables whole and by system columns, weighted
# (GEN_FEATURES=wholerow,syscol):
#
#   - as built, every case passes;
#   - with mutant W01 (the derivation reads a table read whole column by
#     column), the planner check at creation (src/lineage/plan_reads.rs, a
#     test-hooks build) refuses it;
#   - with mutant W02 (the same, the planner check skipped), the generator's
#     perturbation (gen_perturb in gen/lib.sql) catches it.
#
# Needs a build with the test-hooks and test-mutants features.
#
# Usage:
#   PGHOST=localhost PGPORT=28818 PGUSER=postgres ./test/sql/differential/readset-checks.sh [SHAPES] [SEEDS]

set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
shapes="${1:-20}"
seeds="${2:-1 2 3}"
export GEN_FEATURES=wholerow,syscol
fail=0
run() {
  local log
  log="$(mktemp)"
  GEN_PGOPTIONS="${1:+-c pg_tviews.test_mutant=$1}" "$here/gen/run.sh" "$shapes" "$seeds" >"$log" 2>&1
  local status=$?
  cat "$log"
  rm -f "$log"
  return $status
}

if out="$(run "")"; then
  echo "PASS  as built: every case passes"
else
  echo "FAIL  as built:"; grep -E '^(FAIL|ERROR)|PERTURBED|read sets' <<<"$out" | head -5; fail=1
fi
out="$(run W01)"
if grep -q "Check the read sets of tv_" <<<"$out"; then
  echo "PASS  W01: the planner check refused a column-by-column read set of a table read whole"
else
  echo "FAIL  W01: the planner check did not refuse"; fail=1
fi
out="$(run W02)"
if grep -q "PERTURBED" <<<"$out"; then
  echo "PASS  W02: the perturbation caught it"
else
  echo "FAIL  W02: the perturbation did not catch it"; fail=1
fi
exit $fail
